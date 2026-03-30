//! Full-flow example — every ditto interceptor type in one request.
//!
//! Endpoints:
//!
//! ## POST /checkout  (sequential: all interceptor types in order)
//! | seq | Type           | What                                          |
//! |-----|----------------|-----------------------------------------------|
//! |  0  | HTTP entry     | `POST /checkout` (recorded by middleware)     |
//! |  1  | HTTP outbound  | Fetch user profile (jsonplaceholder)          |
//! |  2  | Postgres       | Insert / look up order row                    |
//! |  3  | Redis          | Set idempotency key                           |
//! |  4  | gRPC           | Notify in-process FulfillmentService          |
//! |  5  | `record_io`    | Compute tax (sequential async fn)             |
//! |  6  | `record_io`    | Apply coupon discount (sequential async fn)   |
//! |  7  | spawned task   | Background audit event (spawn_with_ctx)       |
//!
//! ## GET /analyze/:product_id  (parallel fan-out via tokio::join!)
//! | seq | Type           | What                                          |
//! |-----|----------------|-----------------------------------------------|
//! |  0  | HTTP entry     | `GET /analyze/:id` (recorded by middleware)   |
//! | 1-3 | `record_io`    | fetch_price / check_inventory / get_rating    |
//! |     |                | all three run concurrently via tokio::join!   |
//!
//! ## POST /pipeline  (sequential chain: each step consumes previous output)
//! | seq | Type           | What                                          |
//! |-----|----------------|-----------------------------------------------|
//! |  0  | HTTP entry     | `POST /pipeline` (recorded by middleware)     |
//! |  1  | `record_io`    | validate_input → returns cleaned value        |
//! |  2  | `record_io`    | enrich_data (uses step 1 output)              |
//! |  3  | `record_io`    | score_risk (uses step 2 output)               |
//! |  4  | `record_io`    | format_result (uses step 3 output)            |
//!
//! ## POST /checkout-diesel  (Diesel ORM version of checkout)
//! | seq | Type           | What                                          |
//! |-----|----------------|-----------------------------------------------|
//! |  0  | HTTP entry     | `POST /checkout-diesel`                       |
//! |  1  | HTTP outbound  | Fetch user profile                            |
//! |  2  | Diesel/Postgres| Insert / look up order row using Diesel ORM   |
//! |  3  | Redis          | Set idempotency key                           |
//! |  4  | gRPC           | Notify in-process FulfillmentService          |
//! |  5  | `record_io`    | Compute tax                                   |
//! |  6  | `record_io`    | Apply coupon discount                         |
//!
//! # Running
//! ```bash
//! # Record
//! REPLAY_MODE=record cargo run -p full-flow
//!
//! # Replay
//! REPLAY_MODE=replay cargo run -p full-flow
//! ```

use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{get, post},
};
use replay_core::{InteractionStore, MacroStore, ReplayMode};
use replay_interceptors::{grpc::ReplayGrpcLayer, http_server::recording_middleware_with_store};
use replay_macro::record_io;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tonic::transport::Channel;
use uuid::Uuid;

// Diesel imports
use diesel::prelude::*;
use diesel_models::{DieselOrder, NewDieselOrder};
use diesel_models::schema::diesel_orders;

// ── Proto-generated types ─────────────────────────────────────────────────────

pub mod diesel_models;

pub mod fulfillment {
    tonic::include_proto!("fulfillment.v1");
}
use fulfillment::{
    NotifyOrderRequest, NotifyOrderResponse,
    fulfillment_service_client::FulfillmentServiceClient,
    fulfillment_service_server::{FulfillmentService, FulfillmentServiceServer},
};

// ── DB row type ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct OrderRow {
    order_id:        String,
    idempotency_key: String,
    amount:          f64,
    status:          String,
}

// ── Request / Response types ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct CheckoutRequest {
    user_id:         u32,
    amount:          f64,
    idempotency_key: String,
    #[serde(default)]
    coupon_code:     Option<String>,
}

#[derive(Debug, Deserialize)]
struct PipelineRequest {
    value:    String,
    category: String,
}

// ── App state ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    store:           Arc<dyn InteractionStore>,
    db:              Option<replay_compat::sql::Pool>,
    diesel_pool:     Option<replay_compat::bb8_diesel::Bb8Pool>,
    redis:           Option<replay_compat::redis::Connection>,
    grpc_ch:         Channel,
}

// ── record_io functions — sequential (checkout) ───────────────────────────────

/// Compute sales tax.  In record mode the real calculation runs and is stored.
/// In replay mode the stored result is returned without executing the fn body.
#[record_io]
async fn compute_tax(amount: f64) -> f64 {
    (amount * 0.08 * 100.0).round() / 100.0
}

/// Apply coupon discount.
#[record_io]
async fn apply_discount(amount: f64, coupon: String) -> f64 {
    match coupon.as_str() {
        "SAVE10" => (amount * 0.10 * 100.0).round() / 100.0,
        "SAVE20" => (amount * 0.20 * 100.0).round() / 100.0,
        _        => 0.0,
    }
}

/// Background audit event — called inside a spawned task (spawn_with_ctx).
#[record_io]
async fn audit_event(order_id: String, amount: f64) -> String {
    tracing::info!(%order_id, amount, "audit event fired");
    format!("audit:{order_id}")
}

// ── record_io functions — parallel fan-out (analyze) ─────────────────────────

/// Fetch product price from (simulated) pricing service.
#[record_io]
async fn fetch_price(product_id: String) -> f64 {
    // Simulates an external pricing API call
    let base: f64 = product_id.len() as f64 * 4.99;
    (base * 100.0).round() / 100.0
}

/// Check product inventory count.
#[record_io]
async fn check_inventory(product_id: String) -> u32 {
    // Simulates a warehouse inventory lookup
    (product_id.len() as u32) * 7
}

/// Fetch product star rating.
#[record_io]
async fn get_rating(product_id: String) -> f64 {
    // Simulates a reviews-service call
    let hash = product_id.bytes().fold(0u32, |a, b| a.wrapping_add(b as u32));
    3.0 + (hash % 20) as f64 / 10.0
}

// ── record_io functions — sequential chain (pipeline) ────────────────────────

/// Validate and normalise raw input string.
#[record_io]
async fn validate_input(raw: String) -> String {
    raw.trim().to_lowercase()
}

/// Enrich a validated value with contextual metadata.
#[record_io]
async fn enrich_data(value: String, category: String) -> String {
    format!("{category}::{value}")
}

/// Score risk level for an enriched data record.
#[record_io]
async fn score_risk(enriched: String) -> u8 {
    // Simulates an ML inference call
    (enriched.len() % 100) as u8
}

/// Format final pipeline result for the API response.
#[record_io]
async fn format_result(enriched: String, risk_score: u8) -> String {
    if risk_score > 70 {
        format!("HIGH_RISK: {enriched}")
    } else if risk_score > 40 {
        format!("MEDIUM_RISK: {enriched}")
    } else {
        format!("LOW_RISK: {enriched}")
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn health() -> impl IntoResponse {
    let mode = std::env::var("REPLAY_MODE").unwrap_or_else(|_| "off".into());
    Json(json!({ "status": "ok", "mode": mode }))
}

/// POST /checkout
/// Sequential flow: HTTP outbound → DB → Redis → gRPC → record_io × 2 → spawned task
async fn checkout(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CheckoutRequest>,
) -> impl IntoResponse {
    let order_id = Uuid::new_v4().to_string();
    tracing::info!(order_id, user_id = req.user_id, "checkout started");

    // ── seq=1: HTTP outbound ──────────────────────────────────────────────────
    let http_client = replay_compat::http::Client::new();
    let user_name: String = match http_client
        .get(format!(
            "https://jsonplaceholder.typicode.com/users/{}",
            req.user_id
        ))
        .send()
        .await
    {
        Ok(resp) => resp
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| v["name"].as_str().map(str::to_string))
            .unwrap_or_else(|| format!("user-{}", req.user_id)),
        Err(_) => format!("user-{}", req.user_id),
    };

    // ── seq=2: Postgres ───────────────────────────────────────────────────────
    let db_result: Option<String> = if let Some(db) = &state.db {
        let idem_key   = req.idempotency_key.clone();
        let idem_key2  = req.idempotency_key.clone();
        let order_id2  = order_id.clone();
        let amount     = req.amount;

        let existing: Option<OrderRow> = db
            .fetch_optional_as(
                "SELECT order_id, idempotency_key, amount, status \
                 FROM checkout_orders WHERE idempotency_key = $1",
                move |q| q.bind(idem_key),
            )
            .await
            .ok()
            .flatten();

        if let Some(row) = existing {
            tracing::info!("idempotent replay — returning existing order {}", row.order_id);
            Some(row.order_id)
        } else {
            db.execute(
                "INSERT INTO checkout_orders \
                 (order_id, idempotency_key, amount, status) VALUES ($1, $2, $3, $4)",
                move |q| q.bind(order_id2).bind(idem_key2).bind(amount).bind("pending"),
            )
            .await
            .ok();
            Some(order_id.clone())
        }
    } else {
        tracing::warn!("REPLAY_DB_URL not set — skipping DB call");
        Some(order_id.clone())
    };

    let final_order_id = db_result.unwrap_or_else(|| order_id.clone());

    // ── seq=3: Redis ──────────────────────────────────────────────────────────
    if let Some(redis) = &state.redis {
        let key = format!("idempotency:{}", req.idempotency_key);
        if let Err(e) = redis.set(&key, &final_order_id, 300).await {
            tracing::warn!("redis set failed: {e}");
        }
    } else {
        tracing::warn!("REDIS_URL not set — skipping Redis call");
    }

    // ── seq=4: gRPC ───────────────────────────────────────────────────────────
    let instrumented_ch = tower::ServiceBuilder::new()
        .layer(ReplayGrpcLayer::new(state.store.clone()))
        .service(state.grpc_ch.clone());
    let mut grpc_client = FulfillmentServiceClient::new(instrumented_ch);

    let fulfillment_id = grpc_client
        .notify_order(NotifyOrderRequest {
            order_id: final_order_id.clone(),
            amount:   req.amount,
        })
        .await
        .map(|r| r.into_inner().fulfillment_id)
        .unwrap_or_else(|e| {
            tracing::warn!("gRPC notify failed: {e}");
            "unknown".into()
        });

    // ── seq=5: record_io — compute tax ────────────────────────────────────────
    let tax = compute_tax(req.amount).await;

    // ── seq=6: record_io — apply discount ────────────────────────────────────
    let coupon = req.coupon_code.clone().unwrap_or_default();
    let discount = apply_discount(req.amount, coupon).await;

    // ── seq=7: spawned task — background audit (context propagated via spawn_with_ctx) ──
    let audit_order_id = final_order_id.clone();
    let audit_amount   = req.amount;
    replay_compat::tokio::task::spawn(async move {
        audit_event(audit_order_id, audit_amount).await;
    });

    let total = (req.amount + tax - discount) * 100.0 / 100.0;

    (
        StatusCode::OK,
        Json(json!({
            "order_id":       final_order_id,
            "fulfillment_id": fulfillment_id,
            "user":           user_name,
            "amount":         req.amount,
            "tax":            tax,
            "discount":       discount,
            "total":          total,
            "status":         "accepted",
        })),
    )
}

/// GET /analyze/:product_id
/// Parallel fan-out: three record_io functions run concurrently with tokio::join!.
/// All three sequence slots are allocated before any of them execute, so the
/// recorded sequences are deterministic (1, 2, 3) while the async work overlaps.
async fn analyze(
    Path(product_id): Path<String>,
) -> impl IntoResponse {
    tracing::info!(%product_id, "analyze started");

    // seq=1, 2, 3 — all three run in parallel via tokio::join!
    let (price, inventory, rating) = tokio::join!(
        fetch_price(product_id.clone()),
        check_inventory(product_id.clone()),
        get_rating(product_id.clone()),
    );

    let in_stock = inventory > 0;
    let value_score = if price > 0.0 { rating / price * 100.0 } else { 0.0 };

    Json(json!({
        "product_id": product_id,
        "price":       price,
        "inventory":   inventory,
        "in_stock":    in_stock,
        "rating":      rating,
        "value_score": (value_score * 100.0).round() / 100.0,
    }))
}

/// POST /checkout-diesel
/// Diesel ORM version of checkout — demonstrates ditto's Diesel integration.
async fn checkout_diesel(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CheckoutRequest>,
) -> impl IntoResponse {
    let order_id = Uuid::new_v4().to_string();
    tracing::info!(order_id, user_id = req.user_id, "checkout-diesel started");

    // seq=1: HTTP outbound
    let http_client = replay_compat::http::Client::new();
    let user_name: String = match http_client
        .get(format!(
            "https://jsonplaceholder.typicode.com/users/{}",
            req.user_id
        ))
        .send()
        .await
    {
        Ok(resp) => resp
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| v["name"].as_str().map(str::to_string))
            .unwrap_or_else(|| format!("user-{}", req.user_id)),
        Err(_) => format!("user-{}", req.user_id),
    };

    // seq=2: BB8-Diesel/Postgres
    let db_result: Option<String> = if let Some(ref pool) = state.diesel_pool {
        match pool.get().await {
            Ok(mut conn) => {
                let diesel_exec = replay_compat::bb8_diesel::Bb8DieselExecutor::new();

                let idem_key = req.idempotency_key.clone();
                let order_id2 = order_id.clone();
                let amount = req.amount;

                // Check for existing order by idempotency key
                let existing: Option<DieselOrder> = diesel_exec
                    .first_optional(
                        &mut conn,
                        diesel_orders::table.filter(diesel_orders::idempotency_key.eq(idem_key)),
                    )
                    .await
                    .ok()
                    .flatten();

                if let Some(row) = existing {
                    tracing::info!("idempotent replay — returning existing order {}", row.order_id);
                    Some(row.order_id)
                } else {
                    // Insert new order
                    let new_order = NewDieselOrder {
                        order_id: order_id2.clone(),
                        idempotency_key: req.idempotency_key.clone(),
                        amount,
                        status: "pending".to_string(),
                    };

                    let _ = diesel_exec
                        .execute(&mut conn, diesel::insert_into(diesel_orders::table).values(new_order))
                        .await;
                    Some(order_id2)
                }
            }
            Err(e) => {
                tracing::warn!("Failed to get diesel connection from pool: {e}");
                Some(order_id.clone())
            }
        }
    } else {
        tracing::warn!("Diesel connection not available — skipping DB call");
        Some(order_id.clone())
    };

    let final_order_id = db_result.unwrap_or_else(|| order_id.clone());

    // seq=3: Redis
    if let Some(redis) = &state.redis {
        let key = format!("idempotency:{}", req.idempotency_key);
        if let Err(e) = redis.set(&key, &final_order_id, 300).await {
            tracing::warn!("redis set failed: {e}");
        }
    } else {
        tracing::warn!("REDIS_URL not set — skipping Redis call");
    }

    // seq=4: gRPC
    let instrumented_ch = tower::ServiceBuilder::new()
        .layer(ReplayGrpcLayer::new(state.store.clone()))
        .service(state.grpc_ch.clone());
    let mut grpc_client = FulfillmentServiceClient::new(instrumented_ch);

    let fulfillment_id = grpc_client
        .notify_order(NotifyOrderRequest {
            order_id: final_order_id.clone(),
            amount: req.amount,
        })
        .await
        .map(|r| r.into_inner().fulfillment_id)
        .unwrap_or_else(|e| {
            tracing::warn!("gRPC notify failed: {e}");
            "unknown".into()
        });

    // seq=5: compute tax
    let tax = compute_tax(req.amount).await;

    // seq=6: apply discount
    let coupon = req.coupon_code.clone().unwrap_or_default();
    let discount = apply_discount(req.amount, coupon).await;

    let total = (req.amount + tax - discount) * 100.0 / 100.0;

    (
        StatusCode::OK,
        Json(json!({
            "order_id": final_order_id,
            "fulfillment_id": fulfillment_id,
            "user": user_name,
            "amount": req.amount,
            "tax": tax,
            "discount": discount,
            "total": total,
            "status": "accepted",
            "source": "diesel",
        })),
    )
}

/// POST /checkout-bb8-diesel
/// BB8-Diesel version of checkout — demonstrates ditto's async_bb8_diesel integration.
async fn checkout_bb8_diesel(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CheckoutRequest>,
) -> impl IntoResponse {
    let order_id = Uuid::new_v4().to_string();
    tracing::info!(order_id, user_id = req.user_id, "checkout-bb8-diesel started");

    // seq=1: HTTP outbound
    let http_client = replay_compat::http::Client::new();
    let user_name: String = match http_client
        .get(format!(
            "https://jsonplaceholder.typicode.com/users/{}",
            req.user_id
        ))
        .send()
        .await
    {
        Ok(resp) => resp
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| v["name"].as_str().map(str::to_string))
            .unwrap_or_else(|| format!("user-{}", req.user_id)),
        Err(_) => format!("user-{}", req.user_id),
    };

    // seq=2: BB8-Diesel/Postgres
    let db_result: Option<String> = if let Some(ref pool) = state.diesel_pool {
        match pool.get().await {
            Ok(mut conn) => {
                let bb8_exec = replay_compat::bb8_diesel::Bb8DieselExecutor::new();

                let idem_key = req.idempotency_key.clone();
                let order_id2 = order_id.clone();
                let amount = req.amount;

                // Check for existing order by idempotency key
                let existing: Option<DieselOrder> = bb8_exec
                    .first_optional(
                        &mut conn,
                        diesel_orders::table.filter(diesel_orders::idempotency_key.eq(idem_key)),
                    )
                    .await
                    .ok()
                    .flatten();

                if let Some(row) = existing {
                    tracing::info!("bb8-diesel: idempotent replay — returning existing order {}", row.order_id);
                    Some(row.order_id)
                } else {
                    // Insert new order
                    let new_order = NewDieselOrder {
                        order_id: order_id2.clone(),
                        idempotency_key: req.idempotency_key.clone(),
                        amount,
                        status: "pending".to_string(),
                    };

                    let _ = bb8_exec
                        .execute(&mut conn, diesel::insert_into(diesel_orders::table).values(new_order))
                        .await;
                    Some(order_id2)
                }
            }
            Err(e) => {
                tracing::warn!("Failed to get bb8-diesel connection from pool: {e}");
                Some(order_id.clone())
            }
        }
    } else {
        tracing::warn!("BB8-Diesel connection not available — skipping DB call");
        Some(order_id.clone())
    };

    let final_order_id = db_result.unwrap_or_else(|| order_id.clone());

    // seq=3: Redis
    if let Some(redis) = &state.redis {
        let key = format!("idempotency:{}", req.idempotency_key);
        if let Err(e) = redis.set(&key, &final_order_id, 300).await {
            tracing::warn!("redis set failed: {e}");
        }
    } else {
        tracing::warn!("REDIS_URL not set — skipping Redis call");
    }

    // seq=4: gRPC
    let instrumented_ch = tower::ServiceBuilder::new()
        .layer(ReplayGrpcLayer::new(state.store.clone()))
        .service(state.grpc_ch.clone());
    let mut grpc_client = FulfillmentServiceClient::new(instrumented_ch);

    let fulfillment_id = grpc_client
        .notify_order(NotifyOrderRequest {
            order_id: final_order_id.clone(),
            amount: req.amount,
        })
        .await
        .map(|r| r.into_inner().fulfillment_id)
        .unwrap_or_else(|e| {
            tracing::warn!("gRPC notify failed: {e}");
            "unknown".into()
        });

    // seq=5: compute tax
    let tax = compute_tax(req.amount).await;

    // seq=6: apply discount
    let coupon = req.coupon_code.clone().unwrap_or_default();
    let discount = apply_discount(req.amount, coupon).await;

    let total = (req.amount + tax - discount) * 100.0 / 100.0;

    (
        StatusCode::OK,
        Json(json!({
            "order_id": final_order_id,
            "fulfillment_id": fulfillment_id,
            "user": user_name,
            "amount": req.amount,
            "tax": tax,
            "discount": discount,
            "total": total,
            "status": "accepted",
            "source": "bb8-diesel",
        })),
    )
}

/// POST /pipeline
/// Sequential chain: each record_io step consumes the output of the previous one.
/// Demonstrates how record_io captures the full data transformation lineage.
async fn pipeline(
    Json(req): Json<PipelineRequest>,
) -> impl IntoResponse {
    tracing::info!(value = %req.value, category = %req.category, "pipeline started");

    // seq=1: validate raw input
    let validated = validate_input(req.value).await;

    // seq=2: enrich with category context (uses seq=1 output)
    let enriched = enrich_data(validated.clone(), req.category).await;

    // seq=3: score risk level (uses seq=2 output)
    let risk = score_risk(enriched.clone()).await;

    // seq=4: format final result (uses seq=2 + seq=3 outputs)
    let result = format_result(enriched.clone(), risk).await;

    Json(json!({
        "validated":  validated,
        "enriched":   enriched,
        "risk_score": risk,
        "result":     result,
    }))
}

// ── gRPC server (in-process) ──────────────────────────────────────────────────

#[derive(Default)]
struct FulfillmentServer;

#[tonic::async_trait]
impl FulfillmentService for FulfillmentServer {
    async fn notify_order(
        &self,
        request: tonic::Request<NotifyOrderRequest>,
    ) -> Result<tonic::Response<NotifyOrderResponse>, tonic::Status> {
        let req = request.into_inner();
        let fid = format!("fulf_{}", &req.order_id[..8.min(req.order_id.len())]);
        tracing::info!(order_id = %req.order_id, fulfillment_id = %fid, "gRPC: order accepted");
        Ok(tonic::Response::new(NotifyOrderResponse {
            status:         "accepted".into(),
            fulfillment_id: fid,
        }))
    }
}

// ── main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "full_flow=info".into()),
        )
        .init();

    // ── Store ─────────────────────────────────────────────────────────────────
    // We need both Arc<dyn InteractionStore> (for middleware/gRPC/compat) and
    // Arc<dyn MacroStore> (for #[record_io] macro runtime). Both concrete store
    // types implement both traits, so we capture the concrete Arc before upcasting.
    let (store, macro_store): (Arc<dyn InteractionStore>, Arc<dyn MacroStore>) =
        match replay_store::db_url() {
            Some(url) => {
                tracing::info!("connecting to postgres store");
                let s = Arc::new(replay_store::PostgresStore::new(&url).await?);
                (s.clone() as Arc<dyn InteractionStore>, s as Arc<dyn MacroStore>)
            }
            None => {
                tracing::warn!("REPLAY_DB_URL not set — using in-memory store");
                let s = Arc::new(replay_store::InMemoryStore::new());
                (s.clone() as Arc<dyn InteractionStore>, s as Arc<dyn MacroStore>)
            }
        };

    // ── Replay mode ───────────────────────────────────────────────────────────
    let mode = match std::env::var("REPLAY_MODE").as_deref() {
        Ok("record") => ReplayMode::Record,
        Ok("replay") => ReplayMode::Replay,
        _            => ReplayMode::Off,
    };

    // Install replay_compat global (for http::Client, redis::open, sql::connect)
    replay_compat::install(store.clone(), mode.clone());

    // Install replay_core global (for #[record_io] macro-generated code).
    // This is a SEPARATE global from replay_compat — both must be set.
    replay_core::set_global_store(macro_store);

    // ── Postgres pool for DDL (raw sqlx — not intercepted) ───────────────────
    if let Some(url) = replay_store::db_url() {
        let pg = sqlx::PgPool::connect(&url).await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS checkout_orders (
                order_id        TEXT PRIMARY KEY,
                idempotency_key TEXT UNIQUE,
                amount          FLOAT8,
                status          TEXT,
                created_at      TIMESTAMPTZ DEFAULT now()
            )",
        )
        .execute(&pg)
        .await?;
        tracing::info!("checkout_orders table ready");

        // Also create the Diesel orders table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS diesel_orders (
                order_id        TEXT PRIMARY KEY,
                idempotency_key TEXT UNIQUE,
                amount          FLOAT8,
                status          TEXT,
                created_at      TIMESTAMPTZ DEFAULT now()
            )",
        )
        .execute(&pg)
        .await?;
        tracing::info!("diesel_orders table ready");
    }

    // ── DB pool (intercepted by replay_compat) ────────────────────────────────
    let db = match replay_store::db_url() {
        Some(url) => match replay_compat::sql::connect(&url).await {
            Ok(pool) => {
                tracing::info!("db pool connected");
                Some(pool)
            }
            Err(e) => {
                tracing::warn!("db connect failed: {e}");
                None
            }
        },
        None => None,
    };

    // ── BB8 Diesel connection pool ───────────────────────────────────────────
    let diesel_pool = match replay_store::db_url() {
        Some(url) => {
            let manager = replay_compat::bb8_diesel::ConnectionManager::new(url);
            match replay_compat::bb8_diesel::Pool::builder().max_size(5).build(manager).await {
                Ok(pool) => {
                    tracing::info!("bb8-diesel connection pool established");
                    Some(pool)
                }
                Err(e) => {
                    tracing::warn!("bb8-diesel pool build failed: {e}");
                    None
                }
            }
        }
        None => None,
    };

    // ── Redis (intercepted by replay_compat) ──────────────────────────────────
    let redis_url = replay_store::redis_url().unwrap_or_else(|| "redis://127.0.0.1".into());
    let redis = match replay_compat::redis::open(&redis_url) {
        Ok(conn) => {
            tracing::info!(%redis_url, "redis client ready");
            Some(conn)
        }
        Err(e) => {
            tracing::warn!("redis open failed: {e}");
            None
        }
    };

    // ── gRPC server (in-process) ──────────────────────────────────────────────
    let grpc_port = std::env::var("GRPC_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(50061);
    let grpc_addr = format!("127.0.0.1:{grpc_port}");

    let grpc_addr_clone = grpc_addr.clone();
    tokio::spawn(async move {
        tracing::info!("gRPC FulfillmentService listening on {grpc_addr_clone}");
        tonic::transport::Server::builder()
            .add_service(FulfillmentServiceServer::new(FulfillmentServer::default()))
            .serve(grpc_addr_clone.parse().expect("valid addr"))
            .await
            .expect("gRPC server failed");
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // ── gRPC channel (shared, re-wrapped per request with ReplayGrpcLayer) ────
    let grpc_ch = Channel::from_shared(format!("http://{grpc_addr}"))
        .expect("valid uri")
        .connect()
        .await?;

    // ── Axum app ──────────────────────────────────────────────────────────────
    let state = Arc::new(AppState { store: store.clone(), db, diesel_pool, redis, grpc_ch });

    let app = Router::new()
        .route("/health",                 get(health))
        .route("/checkout",               post(checkout))
        .route("/checkout-diesel",        post(checkout_diesel))
        .route("/checkout-bb8-diesel",    post(checkout_bb8_diesel))
        .route("/analyze/:product_id",    get(analyze))
        .route("/pipeline",               post(pipeline))
        .layer(middleware::from_fn_with_state(
            store,
            recording_middleware_with_store,
        ));

    let port = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(3001);
    let addr = format!("0.0.0.0:{port}").parse::<std::net::SocketAddr>()?;

    tracing::info!(%addr, mode = ?mode, "full-flow server listening");
    tracing::info!("endpoints:");
    tracing::info!("  POST http://localhost:{port}/checkout            (sequential: http+db+redis+grpc+fn)");
    tracing::info!("  POST http://localhost:{port}/checkout-diesel     (bb8-diesel version)");
    tracing::info!("  POST http://localhost:{port}/checkout-bb8-diesel (bb8-diesel version)");
    tracing::info!("  GET  http://localhost:{port}/analyze/42          (parallel fan-out: 3x record_io)");
    tracing::info!("  POST http://localhost:{port}/pipeline            (sequential chain: 4x record_io)");

    axum::Server::bind(&addr)
        .serve(app.with_state(state).into_make_service())
        .await?;

    Ok(())
}
