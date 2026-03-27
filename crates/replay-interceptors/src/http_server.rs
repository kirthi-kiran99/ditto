use std::sync::Arc;
use std::time::Instant;

use axum::{
    body::Body,
    extract::State,
    middleware::Next,
    response::Response,
};
use http::Request;
use replay_core::{
    context::with_recording_id, next_interaction_slot, CallStatus, CallType,
    FingerprintBuilder, Interaction, InteractionStore, ReplayMode,
};
use serde_json::json;
use uuid::Uuid;

/// Axum middleware that establishes a `MockContext` for every incoming request.
///
/// In `Record` mode: a fresh `record_id` is generated per request.
/// In `Replay` mode: `record_id` is read from the `x-replay-record-id` header
///   so the harness can tell the service which recording to serve from.
///
/// # Usage
/// ```ignore
/// let app = Router::new()
///     .route("/", get(handler))
///     .layer(axum::middleware::from_fn(recording_middleware));
/// ```
pub async fn recording_middleware(req: Request<Body>, next: Next<Body>) -> Response {
    let mode = current_mode();

    if matches!(mode, ReplayMode::Off) {
        return next.run(req).await;
    }

    let record_id = match mode {
        ReplayMode::Replay => extract_record_id_header(&req).unwrap_or_else(Uuid::new_v4),
        _ => Uuid::new_v4(),
    };

    with_recording_id(record_id, mode, async move { next.run(req).await }).await
}

/// Read `REPLAY_MODE` from the environment (defaults to `Off`).
pub fn current_mode() -> ReplayMode {
    match std::env::var("REPLAY_MODE").as_deref() {
        Ok("record")      => ReplayMode::Record,
        Ok("replay")      => ReplayMode::Replay,
        Ok("passthrough") => ReplayMode::Passthrough,
        _                 => ReplayMode::Off,
    }
}

/// Axum middleware (store-aware variant) that establishes a `MockContext` for
/// every incoming request **and** records the entry-point interaction.
///
/// In `Record` mode:
/// - Generates a fresh `record_id`.
/// - Claims sequence 0 for the entry-point (method + path + response status).
/// - All downstream interceptors (HTTP client, Postgres, Redis) receive
///   sequences 1, 2, 3 …
///
/// In `Replay` mode:
/// - Reads `record_id` from the `x-replay-record-id` header (set by the harness).
/// - Does not write any interaction — the store is used only by interceptors.
///
/// # Usage
/// ```ignore
/// let app = Router::new()
///     .route("/", get(handler))
///     .layer(axum::middleware::from_fn_with_state(
///         store.clone(),
///         recording_middleware_with_store,
///     ));
/// ```
pub async fn recording_middleware_with_store(
    State(store): State<Arc<dyn InteractionStore>>,
    req: Request<Body>,
    next: Next<Body>,
) -> Response {
    let mode = current_mode();

    if matches!(mode, ReplayMode::Off) {
        return next.run(req).await;
    }

    let record_id = match mode {
        ReplayMode::Replay => extract_record_id_header(&req).unwrap_or_else(Uuid::new_v4),
        _ => Uuid::new_v4(),
    };

    let method      = req.method().as_str().to_string();
    let path        = req.uri().path().to_string();
    let fingerprint = FingerprintBuilder::http(&method, &path);
    let mode_clone  = mode.clone();

    with_recording_id(record_id, mode, async move {
        // Claim sequence=0 for the entry-point BEFORE the request is processed
        // so that all downstream interceptors receive sequences 1, 2, 3 …
        let slot = if matches!(mode_clone, ReplayMode::Record) {
            next_interaction_slot(CallType::Http, fingerprint.clone())
        } else {
            None
        };

        let start = Instant::now();
        let resp  = next.run(req).await;
        let elapsed = start.elapsed().as_millis() as u64;

        if let Some(slot) = slot {
            let status_code = resp.status().as_u16();
            let interaction = Interaction {
                id:           Uuid::new_v4(),
                record_id:    slot.record_id,
                parent_id:    None,
                sequence:     slot.sequence,
                call_type:    CallType::Http,
                fingerprint,
                request:      json!({ "method": method, "path": path }),
                response:     json!({ "status": status_code }),
                duration_ms:  elapsed,
                status:       if status_code < 500 {
                                  CallStatus::Completed
                              } else {
                                  CallStatus::Error
                              },
                error:        None,
                recorded_at:  chrono::Utc::now(),
                build_hash:   slot.build_hash,
                service_name: String::new(),
            };
            let _ = store.write(&interaction).await;
        }

        resp
    })
    .await
}

fn extract_record_id_header(req: &Request<Body>) -> Option<Uuid> {
    req.headers()
        .get("x-replay-record-id")
        .and_then(|v: &http::HeaderValue| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok())
}
