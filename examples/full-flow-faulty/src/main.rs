//! Faulty version of full-flow — deliberately broken implementations for
//! every interceptor type so that replaying recordings from `full-flow`
//! produces rich regression and diff output in the ditto UI.
//!
//! ## How to use
//!
//! 1. Record with the correct service:
//!    ```bash
//!    REPLAY_MODE=record cargo run -p full-flow
//!    curl -X POST http://localhost:3001/checkout \
//!      -H 'content-type: application/json' \
//!      -d '{"user_id":1,"amount":99.99,"idempotency_key":"order-001","coupon_code":"SAVE10"}'
//!    curl http://localhost:3001/analyze/widget-42
//!    curl -X POST http://localhost:3001/pipeline \
//!      -H 'content-type: application/json' \
//!      -d '{"value":"  Hello World  ","category":"greeting"}'
//!    ```
//!
//! 2. Stop full-flow, start this faulty version on the SAME port:
//!    ```bash
//!    REPLAY_MODE=replay cargo run -p full-flow-faulty
//!    ```
//!
//! 3. Point ditto-server at http://localhost:3001, open the UI, and hit Replay
//!    on any of the three recordings — you'll see regressions across every layer.
//!
//! ## Intentional bugs (what each interceptor returns vs what was recorded)
//!
//! | Layer          | Recorded (correct)           | Replayed (faulty)                    |
//! |----------------|------------------------------|--------------------------------------|
//! | gRPC status    | "accepted"                   | "queued"  ← different status         |
//! | gRPC fulf. id  | "fulf_<order_id[:8]>"        | "q_<order_id[:6]>"                   |
//! | compute_tax    | amount × 8%                  | amount × 9%  ← wrong rate            |
//! | apply_discount | SAVE10 = 10%, SAVE20 = 20%   | SAVE10 = 5%, SAVE20 = 12%            |
//! | audit_event    | "audit:<order_id>"           | "evt:<order_id>"  ← wrong prefix     |
//! | fetch_price    | len × 4.99                   | len × 3.49  ← underpriced            |
//! | check_inventory| len × 7                      | len × 3  ← under-reported            |
//! | get_rating     | 3.0 + (hash%20)/10           | 2.0 + (hash%15)/10  ← deflated       |
//! | validate_input | trim + lowercase             | trim only (no lowercase)             |
//! | enrich_data    | "category::value"            | "value|category"  ← reversed+sep     |
//! | score_risk     | len % 100                    | (len×2) % 100  ← inflated score      |
//! | format_result  | HIGH>70 / MED>40             | HIGH>50 / MED>25  ← wrong thresholds |

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

// ── Proto-generated types ─────────────────────────────────────────────────────

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
    store:   Arc<dyn InteractionStore>,
    db:      Option<replay_compat::sql::Pool>,
    redis:   Option<replay_compat::redis::Connection>,
    grpc_ch: Channel,
}

// ── FAULTY record_io functions — sequential (checkout) ────────────────────────

/// BUG: uses 9% tax rate instead of 8%.
#[record_io]
async fn compute_tax(amount: f64) -> f64 {
    (amount * 0.09 * 100.0).round() / 100.0
}

/// BUG: SAVE10 gives only 5% off (was 10%), SAVE20 gives 12% (was 20%).
#[record_io]
async fn apply_discount(amount: f64, coupon: String) -> f64 {
    match coupon.as_str() {
        "SAVE10" => (amount * 0.05 * 100.0).round() / 100.0,
        "SAVE20" => (amount * 0.12 * 100.0).round() / 100.0,
        _        => 0.0,
    }
}

/// BUG: returns "evt:" prefix instead of "audit:".
#[record_io]
async fn audit_event(order_id: String, amount: f64) -> String {
    tracing::info!(%order_id, amount, "audit event fired");
    format!("evt:{order_id}")
}

// ── FAULTY record_io functions — parallel fan-out (analyze) ──────────────────

/// BUG: underpriced — uses 3.49× multiplier instead of 4.99×.
#[record_io]
async fn fetch_price(product_id: String) -> f64 {
    let base: f64 = product_id.len() as f64 * 3.49;
    (base * 100.0).round() / 100.0
}

/// BUG: under-reports inventory — uses 3× instead of 7×.
#[record_io]
async fn check_inventory(product_id: String) -> u32 {
    (product_id.len() as u32) * 3
}

/// BUG: deflated ratings — starts at 2.0 with smaller range.
#[record_io]
async fn get_rating(product_id: String) -> f64 {
    let hash = product_id.bytes().fold(0u32, |a, b| a.wrapping_add(b as u32));
    2.0 + (hash % 15) as f64 / 10.0
}

// ── FAULTY record_io functions — sequential chain (pipeline) ─────────────────

/// BUG: trims but does NOT lowercase — preserves original casing.
#[record_io]
async fn validate_input(raw: String) -> String {
    raw.trim().to_string()
}

/// BUG: reverses key/value order and uses "|" separator (was "category::value").
#[record_io]
async fn enrich_data(value: String, category: String) -> String {
    format!("{value}|{category}")
}

/// BUG: doubles the length before modding — inflates risk scores.
#[record_io]
async fn score_risk(enriched: String) -> u8 {
    ((enriched.len() * 2) % 100) as u8
}

/// BUG: lower thresholds — more items incorrectly classified as HIGH_RISK.
#[record_io]
async fn format_result(enriched: String, risk_score: u8) -> String {
    if risk_score > 50 {
        format!("HIGH_RISK: {enriched}")
    } else if risk_score > 25 {
        format!("MEDIUM_RISK: {enriched}")
    } else {
        format!("LOW_RISK: {enriched}")
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn health() -> impl IntoResponse {
    let mode = std::env::var("REPLAY_MODE").unwrap_or_else(|_| "off".into());
    Json(json!({ "status": "ok", "mode": mode, "variant": "faulty" }))
}

async fn checkout(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CheckoutRequest>,
) -> impl IntoResponse {
    let order_id = Uuid::new_v4().to_string();
    tracing::info!(order_id, user_id = req.user_id, "checkout started");

    // seq=1: HTTP outbound — same as correct version
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

    // seq=2: Postgres — same as correct version
    let db_result: Option<String> = if let Some(db) = &state.db {
        let idem_key  = req.idempotency_key.clone();
        let idem_key2 = req.idempotency_key.clone();
        let order_id2 = order_id.clone();
        let amount    = req.amount;

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
        Some(order_id.clone())
    };

    let final_order_id = db_result.unwrap_or_else(|| order_id.clone());

    // seq=3: Redis — same as correct version
    if let Some(redis) = &state.redis {
        let key = format!("idempotency:{}", req.idempotency_key);
        if let Err(e) = redis.set(&key, &final_order_id, 300).await {
            tracing::warn!("redis set failed: {e}");
        }
    }

    // seq=4: gRPC — BUG: FaultyFulfillmentServer returns "queued" + different id format
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

    // seq=5: compute_tax — FAULTY (9% instead of 8%)
    let tax = compute_tax(req.amount).await;

    // seq=6: apply_discount — FAULTY (SAVE10=5% instead of 10%)
    let coupon   = req.coupon_code.clone().unwrap_or_default();
    let discount = apply_discount(req.amount, coupon).await;

    // seq=7: spawned audit — FAULTY ("evt:" prefix instead of "audit:")
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

/// GET /analyze/:product_id — parallel fan-out with faulty record_io functions.
async fn analyze(
    Path(product_id): Path<String>,
) -> impl IntoResponse {
    let (price, inventory, rating) = tokio::join!(
        fetch_price(product_id.clone()),
        check_inventory(product_id.clone()),
        get_rating(product_id.clone()),
    );

    let in_stock    = inventory > 0;
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

/// POST /pipeline — sequential chain with faulty record_io functions.
async fn pipeline(
    Json(req): Json<PipelineRequest>,
) -> impl IntoResponse {
    let validated = validate_input(req.value).await;
    let enriched  = enrich_data(validated.clone(), req.category).await;
    let risk      = score_risk(enriched.clone()).await;
    let result    = format_result(enriched.clone(), risk).await;

    Json(json!({
        "validated":  validated,
        "enriched":   enriched,
        "risk_score": risk,
        "result":     result,
    }))
}

// ── FAULTY gRPC server ────────────────────────────────────────────────────────

#[derive(Default)]
struct FaultyFulfillmentServer;

#[tonic::async_trait]
impl FulfillmentService for FaultyFulfillmentServer {
    async fn notify_order(
        &self,
        request: tonic::Request<NotifyOrderRequest>,
    ) -> Result<tonic::Response<NotifyOrderResponse>, tonic::Status> {
        let req = request.into_inner();
        // BUG 1: status is "queued" instead of "accepted"
        // BUG 2: fulfillment_id uses "q_" prefix and only 6 chars (was "fulf_" + 8 chars)
        let fid = format!("q_{}", &req.order_id[..6.min(req.order_id.len())]);
        tracing::info!(order_id = %req.order_id, fulfillment_id = %fid, "gRPC: order queued (faulty)");
        Ok(tonic::Response::new(NotifyOrderResponse {
            status:         "queued".into(),
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
                .unwrap_or_else(|_| "full_flow_faulty=info".into()),
        )
        .init();

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

    let mode = match std::env::var("REPLAY_MODE").as_deref() {
        Ok("record") => ReplayMode::Record,
        Ok("replay") => ReplayMode::Replay,
        _            => ReplayMode::Off,
    };

    replay_compat::install(store.clone(), mode.clone());
    replay_core::set_global_store(macro_store);

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
    }

    let db = match replay_store::db_url() {
        Some(url) => replay_compat::sql::connect(&url).await.ok(),
        None      => None,
    };

    let redis_url = replay_store::redis_url().unwrap_or_else(|| "redis://127.0.0.1".into());
    let redis     = replay_compat::redis::open(&redis_url).ok();

    // Start the FAULTY in-process gRPC server
    let grpc_port = std::env::var("GRPC_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(50062); // different port from full-flow to avoid conflict
    let grpc_addr = format!("127.0.0.1:{grpc_port}");

    let grpc_addr_clone = grpc_addr.clone();
    tokio::spawn(async move {
        tracing::info!("FAULTY gRPC FulfillmentService on {grpc_addr_clone}");
        tonic::transport::Server::builder()
            .add_service(FulfillmentServiceServer::new(FaultyFulfillmentServer::default()))
            .serve(grpc_addr_clone.parse().expect("valid addr"))
            .await
            .expect("gRPC server failed");
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let grpc_ch = Channel::from_shared(format!("http://{grpc_addr}"))
        .expect("valid uri")
        .connect()
        .await?;

    let state = Arc::new(AppState { store: store.clone(), db, redis, grpc_ch });

    let app = Router::new()
        .route("/health",              get(health))
        .route("/checkout",            post(checkout))
        .route("/analyze/:product_id", get(analyze))
        .route("/pipeline",            post(pipeline))
        .layer(middleware::from_fn_with_state(
            store,
            recording_middleware_with_store,
        ));

    let port = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(3001); // same port as full-flow — ditto-server targets this

    let addr = format!("0.0.0.0:{port}").parse::<std::net::SocketAddr>()?;

    tracing::warn!(%addr, "FAULTY full-flow server listening — for replay testing only");
    tracing::info!("endpoints:");
    tracing::info!("  POST http://localhost:{port}/checkout   → wrong tax/discount/gRPC/audit");
    tracing::info!("  GET  http://localhost:{port}/analyze/42 → wrong price/inventory/rating");
    tracing::info!("  POST http://localhost:{port}/pipeline   → wrong validate/enrich/risk/format");

    axum::Server::bind(&addr)
        .serve(app.with_state(state).into_make_service())
        .await?;

    Ok(())
}
