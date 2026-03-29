use std::sync::Arc;
use std::time::Instant;

use anyhow::anyhow;
use async_trait::async_trait;
use replay_core::{
    next_interaction_slot, CallStatus, CallType, FingerprintBuilder, Interaction, ReplayMode,
};
use replay_store::InteractionStore;
use reqwest::{Request, Response, StatusCode};
use reqwest_middleware::{Middleware, Next, Result as MiddlewareResult};
use serde_json::{json, Value};

/// Headers that are stripped before storing a request.
/// These are volatile (per-request auth tokens, request IDs, etc.)
const REDACT_HEADERS: &[&str] = &[
    "authorization",
    "x-api-key",
    "stripe-signature",
    "x-request-id",
    "x-b3-traceid",
    "x-b3-spanid",
];

// ── middleware ────────────────────────────────────────────────────────────────

pub struct ReplayMiddleware {
    store: Arc<dyn InteractionStore>,
}

impl ReplayMiddleware {
    pub fn new(store: Arc<dyn InteractionStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Middleware for ReplayMiddleware {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut task_local_extensions::Extensions,
        next: Next<'_>,
    ) -> MiddlewareResult<Response> {
        let method = req.method().as_str().to_string();
        let path = req.url().path().to_string();
        let fingerprint = FingerprintBuilder::http(&method, &path);

        let slot = match next_interaction_slot(CallType::Http, fingerprint.clone()) {
            Some(s) => s,
            // No active MockContext — pass through untouched
            None => return next.run(req, extensions).await,
        };

        match slot.mode {
            ReplayMode::Passthrough | ReplayMode::Off => {
                next.run(req, extensions).await
            }

            ReplayMode::Record => {
                let req_body = request_to_json(&req);
                let start    = Instant::now();

                let resp = next.run(req, extensions).await?;
                let elapsed = start.elapsed().as_millis() as u64;

                let status_code = resp.status().as_u16();
                let resp_json   = response_to_json(resp).await;

                let interaction = Interaction {
                    id:           uuid::Uuid::new_v4(),
                    record_id:    slot.record_id,
                    parent_id:    None,
                    sequence:     slot.sequence,
                    call_type:    CallType::Http,
                    fingerprint,
                    request:      req_body,
                    response:     resp_json.clone(),
                    duration_ms:  elapsed,
                    status:       if status_code < 500 {
                                      CallStatus::Completed
                                  } else {
                                      CallStatus::Error
                                  },
                    error:        None,
                    recorded_at:  chrono::Utc::now(),
                    build_hash:   slot.build_hash,
                    service_name: slot.service_name.clone(),
                    tag:          slot.tag.clone(),
                };

                // Fire-and-forget — don't fail the real request if storage fails
                if let Err(e) = self.store.write(&interaction).await {
                    tracing_warn(format!("ditto: failed to store interaction: {e}"));
                }

                Ok(json_to_response(status_code, resp_json))
            }

            // Shadow: mock external HTTP calls from the original recording
            // (same as Replay). Only #[record_io] functions run real code in Shadow mode.
            ReplayMode::Replay | ReplayMode::Shadow => {
                let recorded = self
                    .store
                    .find_match(slot.record_id, CallType::Http, &slot.fingerprint, slot.sequence)
                    .await
                    .map_err(|e| reqwest_middleware::Error::Middleware(anyhow!(e)))?
                    .or_else(|| None); // try fuzzy below if exact misses

                // Try fuzzy match if exact miss
                let recorded = match recorded {
                    Some(r) => r,
                    None => {
                        self.store
                            .find_nearest(slot.record_id, &slot.fingerprint, slot.sequence)
                            .await
                            .map_err(|e| reqwest_middleware::Error::Middleware(anyhow!(e)))?
                            .ok_or_else(|| {
                                reqwest_middleware::Error::Middleware(anyhow!(
                                    "ditto: no recorded response for {} seq={}",
                                    slot.fingerprint,
                                    slot.sequence
                                ))
                            })?
                    }
                };

                let status = recorded.response["status"]
                    .as_u64()
                    .unwrap_or(200) as u16;
                Ok(json_to_response(status, recorded.response))
            }
        }
    }
}

// ── serialisation helpers ─────────────────────────────────────────────────────

pub fn request_to_json(req: &Request) -> Value {
    let headers: serde_json::Map<String, Value> = req
        .headers()
        .iter()
        .filter(|(name, _)| !REDACT_HEADERS.contains(&name.as_str()))
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                Value::String(value.to_str().unwrap_or("").to_string()),
            )
        })
        .collect();

    // reqwest clones of body are unavailable at middleware time;
    // we capture what we can (method, url, headers). Body capture
    // requires the caller to pass it separately or use request cloning.
    json!({
        "method":  req.method().as_str(),
        "url":     req.url().to_string(),
        "headers": headers,
    })
}

/// Consume the response, capture status + body as JSON, return the JSON.
/// The caller must reconstruct a synthetic response from this value.
pub async fn response_to_json(resp: Response) -> Value {
    let status = resp.status().as_u16();
    let headers: serde_json::Map<String, Value> = resp
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                Value::String(value.to_str().unwrap_or("").to_string()),
            )
        })
        .collect();

    let body = match resp.text().await {
        Ok(text) => {
            // Try to parse as JSON; fall back to raw string
            serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text))
        }
        Err(_) => Value::Null,
    };

    json!({
        "status":  status,
        "headers": headers,
        "body":    body,
    })
}

/// Reconstruct a reqwest `Response` from a stored JSON value.
pub fn json_to_response(status_code: u16, value: Value) -> Response {
    let status = StatusCode::from_u16(status_code).unwrap_or(StatusCode::OK);

    let body_json = value.get("body").cloned().unwrap_or(value.clone());
    let body_bytes = match &body_json {
        Value::String(s) => s.as_bytes().to_vec(),
        other => serde_json::to_vec(other).unwrap_or_default(),
    };

    // Build a minimal http::Response and convert to reqwest::Response
    let http_resp = http::Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(body_bytes)
        .expect("valid response");

    reqwest::Response::from(http_resp)
}

// ── factory ───────────────────────────────────────────────────────────────────

/// Create a `reqwest` client pre-wired with the replay middleware.
/// Use this everywhere in the application instead of `reqwest::Client::new()`.
pub fn make_http_client(store: Arc<dyn InteractionStore>) -> reqwest_middleware::ClientWithMiddleware {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("valid reqwest client");

    reqwest_middleware::ClientBuilder::new(client)
        .with(ReplayMiddleware::new(store))
        .build()
}

// ── tiny tracing shim ─────────────────────────────────────────────────────────
// Avoids pulling in the full `tracing` crate as a hard dep in this phase.

fn tracing_warn(msg: String) {
    eprintln!("[ditto WARN] {msg}");
}
