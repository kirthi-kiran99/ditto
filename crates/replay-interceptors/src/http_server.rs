use std::sync::Arc;
use std::time::Instant;

use axum::{
    body::Body,
    extract::State,
    middleware::Next,
    response::Response,
};
use axum::body::{boxed, BoxBody, Full};
use bytes::{Bytes, BytesMut};
use http::Request;
use http_body::Body as HttpBody;
use replay_core::{
    context::{with_recording_id, MockContext, MOCK_CTX},
    next_interaction_slot, CallStatus, CallType,
    FingerprintBuilder, Interaction, InteractionStore, ReplayMode,
};
use serde_json::{json, Value};
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
        ReplayMode::Replay => extract_header_uuid(&req, "x-replay-record-id").unwrap_or_else(Uuid::new_v4),
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

/// Read `REPLAY_TAG` from the environment.
pub fn current_tag() -> String {
    std::env::var("REPLAY_TAG").unwrap_or_default()
}

/// Read `REPLAY_SERVICE_NAME` from the environment.
/// Falls back to the executable filename (e.g. "full-flow") when not set.
pub fn current_service_name() -> String {
    std::env::var("REPLAY_SERVICE_NAME").unwrap_or_else(|_| {
        std::env::args()
            .next()
            .as_deref()
            .and_then(|p| std::path::Path::new(p).file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string()
    })
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

    // Shadow mode: harness sends both X-Replay-Record-Id (original) and
    // X-Ditto-Capture-Id (temp write target for #[record_io] results).
    let capture_id = extract_header_uuid(&req, "x-ditto-capture-id");

    let (record_id, effective_mode) = match mode {
        ReplayMode::Replay if capture_id.is_some() => {
            // Harness wants per-interaction capture → use Shadow mode.
            let rid = extract_header_uuid(&req, "x-replay-record-id").unwrap_or_else(Uuid::new_v4);
            (rid, ReplayMode::Shadow)
        }
        ReplayMode::Replay => {
            let rid = extract_header_uuid(&req, "x-replay-record-id").unwrap_or_else(Uuid::new_v4);
            (rid, ReplayMode::Replay)
        }
        _ => (Uuid::new_v4(), mode),
    };

    let method      = req.method().as_str().to_string();
    let path        = req.uri().path().to_string();
    let fingerprint = FingerprintBuilder::http(&method, &path);
    let mode_clone  = effective_mode.clone();

    let tag          = current_tag();
    let service_name = current_service_name();

    let mut ctx = if let Some(cid) = capture_id {
        MockContext::shadow(record_id, cid)
    } else {
        MockContext::with_id(record_id, effective_mode)
    };
    ctx.tag          = tag;
    ctx.service_name = service_name;

    MOCK_CTX.scope(ctx, async move {
        // In Record mode only: buffer the request body so we can store it and
        // replay it later.  We must reconstruct the request with the same bytes
        // so the downstream handler still receives the body.
        let (req, req_body_json) = if matches!(mode_clone, ReplayMode::Record) {
            let (parts, body) = req.into_parts();
            let body_bytes    = collect_request_body(body).await;
            let body_json: Value = serde_json::from_slice(&body_bytes)
                .unwrap_or(Value::Null);
            // Reconstruct request with the buffered body.
            let rebuilt = Request::from_parts(parts, Body::from(body_bytes));
            (rebuilt, body_json)
        } else {
            (req, Value::Null)
        };

        // Claim sequence=0 for the entry-point BEFORE the request is processed
        // so that all downstream interceptors receive sequences 1, 2, 3 …
        //
        // In Shadow mode we still claim seq=0 to keep the counter aligned with
        // the original recording — but we do NOT write an interaction for it.
        let slot = if matches!(mode_clone, ReplayMode::Record | ReplayMode::Shadow) {
            next_interaction_slot(CallType::Http, fingerprint.clone())
        } else {
            None
        };

        let start   = Instant::now();
        let resp    = next.run(req).await;
        let elapsed = start.elapsed().as_millis() as u64;

        if let Some(slot) = slot {
            // Only write the entry-point interaction in Record mode.
            if matches!(slot.mode, ReplayMode::Record) {
                let status_code = resp.status().as_u16();

                // Buffer response body so we can store it and reconstruct the response.
                let (parts, body) = resp.into_parts();
                let body_bytes = collect_body(body).await;
                let body_json: Value = serde_json::from_slice(&body_bytes)
                    .unwrap_or(Value::Null);

                let mut response_json = json!({ "status": status_code });
                if !body_json.is_null() {
                    response_json["body"] = body_json;
                }

                // Build the stored request object: method + path + body (if present).
                let mut request_json = json!({ "method": method, "path": path });
                if !req_body_json.is_null() {
                    request_json["body"] = req_body_json;
                }

                let interaction = Interaction {
                    id:           Uuid::new_v4(),
                    record_id:    slot.record_id,
                    parent_id:    None,
                    sequence:     slot.sequence,
                    call_type:    CallType::Http,
                    fingerprint,
                    request:      request_json,
                    response:     response_json,
                    duration_ms:  elapsed,
                    status:       if status_code < 500 {
                                      CallStatus::Completed
                                  } else {
                                      CallStatus::Error
                                  },
                    error:        None,
                    recorded_at:  chrono::Utc::now(),
                    build_hash:   slot.build_hash,
                    service_name: slot.service_name,
                    tag:          slot.tag,
                };
                let _ = store.write(&interaction).await;

                // Reconstruct response with the buffered body.
                Response::from_parts(parts, boxed(Full::from(body_bytes)))
            } else {
                resp
            }
        } else {
            resp
        }
    })
    .await
}

/// Drain the request `Body` (axum 0.6 = hyper::Body, which is Unpin) into `Bytes`.
async fn collect_request_body(mut body: Body) -> Bytes {
    use std::pin::Pin;
    let mut buf = BytesMut::new();
    while let Some(result) =
        std::future::poll_fn(|cx| Pin::new(&mut body).poll_data(cx)).await
    {
        if let Ok(chunk) = result {
            buf.extend_from_slice(&chunk);
        }
    }
    buf.freeze()
}

/// Drain a `BoxBody` into `Bytes` without requiring `Send`.
async fn collect_body(mut body: BoxBody) -> Bytes {
    use std::pin::Pin;
    let mut buf = BytesMut::new();
    while let Some(result) =
        std::future::poll_fn(|cx| Pin::new(&mut body).poll_data(cx)).await
    {
        if let Ok(chunk) = result {
            buf.extend_from_slice(&chunk);
        }
    }
    buf.freeze()
}

fn extract_header_uuid(req: &Request<Body>, name: &str) -> Option<Uuid> {
    req.headers()
        .get(name)
        .and_then(|v: &http::HeaderValue| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok())
}
