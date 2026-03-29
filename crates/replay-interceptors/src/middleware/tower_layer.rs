//! Framework-agnostic Tower [`Layer`] for replay context injection.
//!
//! Works with Axum, `tower-http`, raw `hyper`, or any framework that accepts
//! Tower middleware.  Users of Axum can continue using the existing
//! [`crate::http_server::recording_middleware`] helpers; this layer is the
//! portable alternative for other stacks.
//!
//! # Usage — context injection only
//! ```ignore
//! use replay_interceptors::middleware::RecordingLayer;
//! use replay_core::ReplayMode;
//! use tower::ServiceBuilder;
//!
//! let service = ServiceBuilder::new()
//!     .layer(RecordingLayer::new(ReplayMode::Record))
//!     .service(my_handler);
//! ```
//!
//! # Usage — with entry-point recording
//! ```ignore
//! let service = ServiceBuilder::new()
//!     .layer(RecordingLayer::with_store(store.clone(), ReplayMode::Record))
//!     .service(my_handler);
//! ```

use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Instant,
};

use http::{Request, Response};
use replay_core::{
    next_interaction_slot, CallStatus, CallType, FingerprintBuilder, Interaction,
    InteractionStore, MockContext, ReplayMode, MOCK_CTX,
};
use tower::{Layer, Service};
use uuid::Uuid;

// ── RecordingLayer ────────────────────────────────────────────────────────────

/// A framework-agnostic Tower [`Layer`] that injects a [`MockContext`] for
/// every incoming request.
///
/// Two constructors:
/// - [`RecordingLayer::new`] — context only, no store interaction.
/// - [`RecordingLayer::with_store`] — context + entry-point interaction at
///   sequence 0 (all downstream interceptors claim sequences 1, 2, …).
#[derive(Clone)]
pub struct RecordingLayer {
    store: Option<Arc<dyn InteractionStore>>,
    mode:  ReplayMode,
}

impl RecordingLayer {
    /// Context-only variant.  Downstream interceptors find the store via the
    /// global store (registered with `replay_core::set_global_store()`).
    pub fn new(mode: ReplayMode) -> Self {
        Self { store: None, mode }
    }

    /// Entry-point recording variant.  Writes the inbound request at sequence 0
    /// in Record mode; downstream interceptors claim sequences 1, 2, 3 …
    pub fn with_store(store: Arc<dyn InteractionStore>, mode: ReplayMode) -> Self {
        Self { store: Some(store), mode }
    }
}

impl<S> Layer<S> for RecordingLayer {
    type Service = RecordingService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RecordingService {
            inner,
            store: self.store.clone(),
            mode:  self.mode.clone(),
        }
    }
}

// ── RecordingService ──────────────────────────────────────────────────────────

/// Tower [`Service`] produced by [`RecordingLayer`].
#[derive(Clone)]
pub struct RecordingService<S> {
    inner: S,
    store: Option<Arc<dyn InteractionStore>>,
    mode:  ReplayMode,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for RecordingService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    S::Error:  Send + 'static,
    ReqBody:   Send + 'static,
    ResBody:   Send + 'static,
{
    type Response = Response<ResBody>;
    type Error    = S::Error;
    type Future   =
        Pin<Box<dyn Future<Output = Result<Response<ResBody>, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let mode  = self.mode.clone();
        let store = self.store.clone();
        // Clone the inner service — standard Tower pattern for moving a service
        // into an async block without consuming `self`.
        let mut inner = self.inner.clone();

        Box::pin(async move {
            if matches!(mode, ReplayMode::Off) {
                return inner.call(req).await;
            }

            let record_id = if matches!(mode, ReplayMode::Replay) {
                extract_record_id_header(&req).unwrap_or_else(Uuid::new_v4)
            } else {
                Uuid::new_v4()
            };

            let is_record = matches!(mode, ReplayMode::Record);
            let ctx = MockContext::with_id(record_id, mode);

            MOCK_CTX
                .scope(ctx, async move {
                    if let (Some(store), true) = (store, is_record) {
                        // Record mode with a store: claim sequence 0 before the
                        // handler runs so downstream interceptors get 1, 2, 3 …
                        let method = req.method().to_string();
                        let path   = req.uri().path().to_string();
                        let fp     = FingerprintBuilder::http(&method, &path);
                        let slot   = next_interaction_slot(CallType::Http, fp.clone());

                        let start      = Instant::now();
                        let resp       = inner.call(req).await;
                        let ms         = start.elapsed().as_millis() as u64;
                        // Extract status before any await so &resp doesn't
                        // cross an await point (which would require ResBody: Sync).
                        let status_opt = resp.as_ref().ok().map(|r| r.status().as_u16());

                        if let (Some(slot), Some(status)) = (slot, status_opt) {
                            let _ = store
                                .write(&Interaction {
                                    id:           Uuid::new_v4(),
                                    record_id:    slot.record_id,
                                    parent_id:    None,
                                    sequence:     slot.sequence,
                                    call_type:    CallType::Http,
                                    fingerprint:  fp,
                                    request:      serde_json::json!({
                                        "method": method,
                                        "path":   path,
                                    }),
                                    response:     serde_json::json!({
                                        "status": status,
                                    }),
                                    duration_ms:  ms,
                                    status:       if status < 500 {
                                        CallStatus::Completed
                                    } else {
                                        CallStatus::Error
                                    },
                                    error:        None,
                                    recorded_at:  chrono::Utc::now(),
                                    build_hash:   slot.build_hash,
                                    service_name: slot.service_name.clone(),
                                    tag:          slot.tag.clone(),
                                })
                                .await;
                        }
                        resp
                    } else {
                        inner.call(req).await
                    }
                })
                .await
        })
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn extract_record_id_header<B>(req: &Request<B>) -> Option<Uuid> {
    req.headers()
        .get("x-replay-record-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok())
}
