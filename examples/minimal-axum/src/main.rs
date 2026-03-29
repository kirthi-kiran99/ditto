//! Minimal Axum example — full record/replay stack in ~150 lines.
//!
//! Demonstrates:
//! - `replay_compat::install()` at startup
//! - Axum entry-point middleware via `recording_middleware`
//! - Instrumented HTTP client (`replay_compat::http::Client`)
//! - Three routes: health check, user fetch (outbound HTTP), echo (no I/O)
//!
//! # Running in Record mode
//! ```bash
//! REPLAY_MODE=record cargo run -p minimal-axum
//! ```
//!
//! # Running in Replay mode (after at least one recording exists)
//! ```bash
//! REPLAY_MODE=replay cargo run -p minimal-axum
//! ```

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::Path,
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::get,
};
use replay_core::{InteractionStore, ReplayMode};
use replay_interceptors::http_server::recording_middleware_with_store;
use replay_store::{InMemoryStore, PostgresStore};
use serde::{Deserialize, Serialize};

// ── response types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct Health {
    status: &'static str,
    mode:   String,
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn current_replay_mode() -> ReplayMode {
    match std::env::var("REPLAY_MODE").as_deref() {
        Ok("record")  => ReplayMode::Record,
        Ok("replay")  => ReplayMode::Replay,
        _             => ReplayMode::Off,
    }
}

// ── handlers ──────────────────────────────────────────────────────────────────

/// GET /health — no outbound calls, returns the current replay mode.
async fn health() -> impl IntoResponse {
    let mode = std::env::var("REPLAY_MODE").unwrap_or_else(|_| "off".into());
    Json(Health { status: "ok", mode })
}

/// GET /users/:id — fetches a user from a downstream API.
///
/// In Record mode the real HTTP call is made and written to the store.
/// In Replay mode the stored response is returned with no network activity.
async fn get_user(Path(id): Path<String>) -> impl IntoResponse {
    // `replay_compat::http::Client` is a drop-in for `reqwest::Client`.
    let client = replay_compat::http::Client::new();

    let result = client
        .get(format!("https://jsonplaceholder.typicode.com/users/{id}"))
        .send()
        .await;

    match result {
        Ok(resp) if resp.status().is_success() => {
            match resp.json::<serde_json::Value>().await {
                Ok(body) => (StatusCode::OK, Json(body)).into_response(),
                Err(_)   => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Ok(resp) => StatusCode::from_u16(resp.status().as_u16())
            .unwrap_or(StatusCode::BAD_GATEWAY)
            .into_response(),
        Err(_) => StatusCode::BAD_GATEWAY.into_response(),
    }
}

/// GET /echo/:message — no outbound calls, just echoes the path segment.
async fn echo(Path(message): Path<String>) -> impl IntoResponse {
    Json(serde_json::json!({ "echo": message }))
}

// ── router ────────────────────────────────────────────────────────────────────

fn build_app(store: Arc<dyn InteractionStore>) -> Router {
    // Install replay_compat so Client::new() finds the store + mode.
    replay_compat::install(store.clone(), current_replay_mode());

    Router::new()
        .route("/health",        get(health))
        .route("/users/:id",     get(get_user))
        .route("/echo/:message", get(echo))
        .layer(middleware::from_fn_with_state(
            store,
            recording_middleware_with_store,
        ))
}

// ── main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let store: Arc<dyn InteractionStore> = match replay_store::db_url() {
        Some(url) => {
            println!("connecting to postgres store at {url}");
            Arc::new(
                PostgresStore::new(&url)
                    .await
                    .unwrap_or_else(|e| { eprintln!("DB connection failed: {e}"); std::process::exit(1); }),
            )
        }
        None => {
            println!("REPLAY_DB_URL not set — using in-memory store (recordings won't persist)");
            Arc::new(InMemoryStore::new())
        }
    };

    let app  = build_app(store);
    let addr = "0.0.0.0:3000".parse::<std::net::SocketAddr>().unwrap();
    println!("listening on http://{addr}  (REPLAY_MODE={})",
             std::env::var("REPLAY_MODE").unwrap_or_else(|_| "off".into()));

    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
