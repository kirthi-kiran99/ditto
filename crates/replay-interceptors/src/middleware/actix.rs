//! Actix-web 4 middleware that injects a [`replay_core::MockContext`] into
//! every incoming request.
//!
//! # Usage
//! ```ignore
//! use replay_interceptors::middleware::actix::ReplayActixMiddleware;
//! use replay_core::ReplayMode;
//!
//! HttpServer::new(move || {
//!     App::new()
//!         .wrap(ReplayActixMiddleware::new(store.clone(), ReplayMode::Record))
//!         .service(payment_handler)
//! })
//! .bind("0.0.0.0:8080")?
//! .run()
//! .await
//! ```

use std::{
    future::{ready, Ready},
    rc::Rc,
    sync::Arc,
};

use actix_web::{
    dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform},
    Error,
};
use futures_util::future::LocalBoxFuture;
use replay_core::{InteractionStore, MockContext, ReplayMode, MOCK_CTX};
use uuid::Uuid;

// ── Transform (factory) ───────────────────────────────────────────────────────

/// Actix-web middleware factory that injects [`MockContext`] for every request.
///
/// In Record mode a fresh `record_id` is generated per request.
/// In Replay mode `record_id` is read from the `x-replay-record-id` header.
pub struct ReplayActixMiddleware {
    store: Option<Arc<dyn InteractionStore>>,
    mode:  ReplayMode,
}

impl ReplayActixMiddleware {
    /// Context-only variant — no entry-point interaction written.
    pub fn new(mode: ReplayMode) -> Self {
        Self { store: None, mode }
    }

    /// Context + entry-point recording variant.
    pub fn with_store(store: Arc<dyn InteractionStore>, mode: ReplayMode) -> Self {
        Self { store: Some(store), mode }
    }
}

impl<S, B> Transform<S, ServiceRequest> for ReplayActixMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response    = ServiceResponse<B>;
    type Error       = Error;
    type Transform   = ReplayActixService<S>;
    type InitError   = ();
    type Future      = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(ReplayActixService {
            service: Rc::new(service),
            store:   self.store.clone(),
            mode:    self.mode.clone(),
        }))
    }
}

// ── Service (per-request) ─────────────────────────────────────────────────────

pub struct ReplayActixService<S> {
    service: Rc<S>,
    store:   Option<Arc<dyn InteractionStore>>,
    mode:    ReplayMode,
}

impl<S, B> Service<ServiceRequest> for ReplayActixService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error    = Error;
    type Future   = LocalBoxFuture<'static, Result<ServiceResponse<B>, Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let svc   = Rc::clone(&self.service);
        let mode  = self.mode.clone();
        let _store = self.store.clone(); // kept for future entry-point recording

        Box::pin(async move {
            if matches!(mode, ReplayMode::Off) {
                return svc.call(req).await;
            }

            let record_id = if matches!(mode, ReplayMode::Replay) {
                extract_record_id_header(&req).unwrap_or_else(Uuid::new_v4)
            } else {
                Uuid::new_v4()
            };

            let ctx = MockContext::with_id(record_id, mode);
            MOCK_CTX.scope(ctx, svc.call(req)).await
        })
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn extract_record_id_header(req: &ServiceRequest) -> Option<Uuid> {
    req.headers()
        .get("x-replay-record-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok())
}
