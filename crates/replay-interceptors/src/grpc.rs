//! gRPC client interceptor via tonic/tower.
//!
//! Wraps any tonic channel with a tower `Layer` that intercepts every unary
//! RPC call. Protobuf bodies are stored as base64-encoded bytes so no schema
//! information is needed at record time. On replay the exact byte payload is
//! returned, which tonic decodes back to the expected message type.
//!
//! # Usage
//! ```ignore
//! use tonic::transport::Channel;
//! use replay_interceptors::grpc::ReplayGrpcLayer;
//!
//! let channel = Channel::from_static("http://[::1]:50051")
//!     .connect()
//!     .await?;
//!
//! let channel = tower::ServiceBuilder::new()
//!     .layer(ReplayGrpcLayer::new(store.clone()))
//!     .service(channel);
//!
//! let client = PaymentServiceClient::new(channel);
//! ```

use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use bytes::Bytes;
use http::{Request, Response};
use http_body::Body as HttpBody;
use replay_core::{
    next_interaction_slot, CallStatus, CallType, FingerprintBuilder, Interaction, ReplayMode,
};
use replay_store::InteractionStore;
use serde_json::json;
use tower::{Layer, Service};
use uuid::Uuid;

// ── Layer ─────────────────────────────────────────────────────────────────────

/// tower `Layer` that wraps any gRPC channel with replay interception.
#[derive(Clone)]
pub struct ReplayGrpcLayer {
    store: Arc<dyn InteractionStore>,
}

impl ReplayGrpcLayer {
    pub fn new(store: Arc<dyn InteractionStore>) -> Self {
        Self { store }
    }
}

impl<S> Layer<S> for ReplayGrpcLayer {
    type Service = ReplayGrpcService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ReplayGrpcService {
            inner,
            store: self.store.clone(),
        }
    }
}

// ── Service ───────────────────────────────────────────────────────────────────

/// tower `Service` that intercepts individual gRPC calls.
///
/// The request body is fixed to `tonic::body::BoxBody` because that is what
/// tonic channels always produce. The response body remains generic so any
/// downstream body type is accepted.
#[derive(Clone)]
pub struct ReplayGrpcService<S> {
    inner: S,
    store: Arc<dyn InteractionStore>,
}

impl<S, ResBody> Service<Request<tonic::body::BoxBody>> for ReplayGrpcService<S>
where
    S: Service<Request<tonic::body::BoxBody>, Response = Response<ResBody>>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    S::Error: std::fmt::Debug + Send,
    ResBody: HttpBody + Send + 'static,
    ResBody::Data: Into<Bytes> + Send,
    ResBody::Error: std::fmt::Debug,
{
    type Response = Response<tonic::body::BoxBody>;
    type Error    = S::Error;
    type Future   = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<tonic::body::BoxBody>) -> Self::Future {
        // gRPC path is "/package.Service/Method" — already stable, no normalization needed
        let path        = req.uri().path().to_string();
        let fingerprint = FingerprintBuilder::grpc(&path);
        let slot        = next_interaction_slot(CallType::Grpc, fingerprint.clone());
        let store       = self.store.clone();

        // Clone inner for use in the async block
        let mut inner = self.inner.clone();
        // Replace inner with the clone (required for tower correctness)
        std::mem::swap(&mut self.inner, &mut inner);

        Box::pin(async move {
            match slot.as_ref().map(|s| &s.mode) {
                Some(ReplayMode::Replay) | Some(ReplayMode::Shadow) => {
                    let slot = slot.unwrap();

                    let stored = store
                        .find_match(
                            slot.record_id,
                            CallType::Grpc,
                            &slot.fingerprint,
                            slot.sequence,
                        )
                        .await
                        .ok()
                        .flatten();

                    // Fuzzy fallback
                    let stored = match stored {
                        Some(i) => Some(i),
                        None => store
                            .find_nearest(slot.record_id, &slot.fingerprint, slot.sequence)
                            .await
                            .ok()
                            .flatten(),
                    };

                    match stored {
                        Some(interaction) => {
                            // Decode the stored base64 response bytes and
                            // wrap in a tonic BoxBody
                            let b64 = interaction.response["body_b64"]
                                .as_str()
                                .unwrap_or("");
                            let body_bytes = B64.decode(b64).unwrap_or_default();

                            let response = build_grpc_response(body_bytes);
                            Ok(response)
                        }
                        None => {
                            // No recording — fall through to real call
                            let resp = inner.call(req).await?;
                            Ok(box_response(resp).await)
                        }
                    }
                }

                Some(ReplayMode::Record) => {
                    // Collect request body bytes for storage
                    let (parts, body) = req.into_parts();
                    let req_bytes = collect_body(body).await;
                    let req_b64   = B64.encode(&req_bytes);

                    // Reconstruct request with buffered body
                    let new_req = Request::from_parts(
                        parts,
                        tonic::body::BoxBody::new(
                            http_body::Full::new(Bytes::from(req_bytes.clone()))
                                .map_err(|e| tonic::Status::internal(format!("{e:?}"))),
                        ),
                    );

                    let start = Instant::now();
                    let resp  = inner.call(new_req).await?;
                    let elapsed = start.elapsed().as_millis() as u64;

                    let status_code = resp.status().as_u16();
                    let (resp_parts, resp_body) = resp.into_parts();
                    let resp_bytes = collect_body(resp_body).await;
                    let resp_b64   = B64.encode(&resp_bytes);

                    let slot = slot.unwrap();
                    let interaction = Interaction {
                        id:           Uuid::new_v4(),
                        record_id:    slot.record_id,
                        parent_id:    None,
                        sequence:     slot.sequence,
                        call_type:    CallType::Grpc,
                        fingerprint,
                        request:      json!({
                            "path":     path,
                            "body_b64": req_b64,
                        }),
                        response:     json!({
                            "status":   status_code,
                            "body_b64": resp_b64,
                        }),
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

                    // Reconstruct original response
                    let rebuilt = Response::from_parts(
                        resp_parts,
                        tonic::body::BoxBody::new(
                            http_body::Full::new(Bytes::from(resp_bytes))
                                .map_err(|e| tonic::Status::internal(format!("{e:?}"))),
                        ),
                    );
                    Ok(rebuilt)
                }

                _ => {
                    // Passthrough / Off — no interception
                    let resp = inner.call(req).await?;
                    Ok(box_response(resp).await)
                }
            }
        })
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Drain an [`HttpBody`] into a byte vector without requiring `Unpin`.
///
/// Uses `pin_mut!` to pin the body on the stack, then `poll_fn` to drive
/// `poll_data` directly — no intermediate stream allocation needed.
async fn collect_body<B>(body: B) -> Vec<u8>
where
    B: HttpBody + Send,
    B::Data: Into<Bytes>,
    B::Error: std::fmt::Debug,
{
    futures_util::pin_mut!(body);
    let mut bytes = Vec::new();
    loop {
        let chunk = std::future::poll_fn(|cx| body.as_mut().poll_data(cx)).await;
        match chunk {
            Some(Ok(data))  => bytes.extend_from_slice(&data.into()),
            Some(Err(_)) | None => break,
        }
    }
    bytes
}

fn build_grpc_response(body_bytes: Vec<u8>) -> Response<tonic::body::BoxBody> {
    Response::builder()
        .status(200)
        .header("content-type", "application/grpc")
        .body(tonic::body::BoxBody::new(
            http_body::Full::new(Bytes::from(body_bytes))
                .map_err(|e| tonic::Status::internal(format!("{e:?}"))),
        ))
        .expect("valid grpc response")
}

async fn box_response<B>(resp: Response<B>) -> Response<tonic::body::BoxBody>
where
    B: HttpBody + Send + 'static,
    B::Data: Into<Bytes> + Send,
    B::Error: std::fmt::Debug,
{
    let (parts, body) = resp.into_parts();
    let bytes = collect_body(body).await;
    Response::from_parts(
        parts,
        tonic::body::BoxBody::new(
            http_body::Full::new(Bytes::from(bytes))
                .map_err(|e| tonic::Status::internal(format!("{e:?}"))),
        ),
    )
}
