use std::sync::Arc;
use std::time::Instant;

use uuid::Uuid;

use crate::{
    context::next_interaction_slot,
    interceptor::{ReplayError, ReplayInterceptor},
    interaction::Interaction,
    store::InteractionStore,
    types::{CallStatus, ReplayMode},
};

// ── runner ────────────────────────────────────────────────────────────────────

/// Drives a [`ReplayInterceptor`] through the record/replay lifecycle.
///
/// The runner is the single place that touches the store and the ambient
/// [`crate::MockContext`]. Interceptors implement call-type-specific logic
/// (fingerprinting, normalisation, execution); the runner handles sequencing,
/// store I/O, and mode switching.
///
/// # Usage
/// ```ignore
/// let runner = InterceptorRunner::new(MyInterceptor::new(client), store.clone());
/// let response: MyResponse = runner.run(my_request).await?;
/// ```
pub struct InterceptorRunner<I: ReplayInterceptor> {
    interceptor: I,
    store:       Arc<dyn InteractionStore>,
}

impl<I: ReplayInterceptor> InterceptorRunner<I> {
    pub fn new(interceptor: I, store: Arc<dyn InteractionStore>) -> Self {
        Self { interceptor, store }
    }

    /// Execute the call through the mode determined by the ambient context:
    ///
    /// | Mode           | Store I/O | Calls `execute()` |
    /// |----------------|-----------|-------------------|
    /// | No context     | none      | yes               |
    /// | Record         | write     | yes               |
    /// | Replay         | read      | **no**            |
    /// | Passthrough    | none      | yes               |
    /// | Off            | none      | yes               |
    pub async fn run(&self, req: I::Request) -> Result<I::Response, I::Error> {
        let fp   = self.interceptor.fingerprint(&req);
        let slot = next_interaction_slot(self.interceptor.call_type(), fp.clone());

        let Some(slot) = slot else {
            // No active MockContext — forward to the real client silently.
            return self.interceptor.execute(req).await;
        };

        match slot.mode {
            // ── record ────────────────────────────────────────────────────────
            ReplayMode::Record => {
                let norm_req = self.interceptor.normalize_request(&req);
                let start    = Instant::now();
                let res      = self.interceptor.execute(req).await?;
                let duration = start.elapsed().as_millis() as u64;
                let norm_res = self.interceptor.normalize_response(&res);

                let interaction = Interaction {
                    id:           Uuid::new_v4(),
                    record_id:    slot.record_id,
                    parent_id:    None,
                    sequence:     slot.sequence,
                    call_type:    self.interceptor.call_type(),
                    fingerprint:  fp,
                    request:      norm_req,
                    response:     norm_res,
                    duration_ms:  duration,
                    status:       CallStatus::Completed,
                    error:        None,
                    recorded_at:  chrono::Utc::now(),
                    build_hash:   slot.build_hash,
                    service_name: String::new(),
                };

                // Fire-and-forget — a store failure must never fail the real request.
                if let Err(e) = self.store.write(&interaction).await {
                    eprintln!("[ditto WARN] runner: store write failed: {e}");
                }

                Ok(res)
            }

            // ── replay / shadow ───────────────────────────────────────────────
            // Shadow: external deps (HTTP, DB, Redis, gRPC) are still mocked from
            // the original recording — same as Replay.  Only #[record_io] functions
            // differ (they execute real code via the macro, not through this runner).
            ReplayMode::Replay | ReplayMode::Shadow => {
                // Tier 1: exact match on record_id + call_type + fingerprint + sequence
                let stored = self
                    .store
                    .find_match(
                        slot.record_id,
                        self.interceptor.call_type(),
                        &slot.fingerprint,
                        slot.sequence,
                    )
                    .await
                    .map_err(|e| I::Error::from(ReplayError::Store(e.to_string())))?;

                // Tier 2: fuzzy / nearest-sequence fallback
                let stored = match stored {
                    Some(i) => i,
                    None => self
                        .store
                        .find_nearest(slot.record_id, &slot.fingerprint, slot.sequence)
                        .await
                        .map_err(|e| I::Error::from(ReplayError::Store(e.to_string())))?
                        .ok_or_else(|| {
                            I::Error::from(ReplayError::NotFound {
                                fingerprint: slot.fingerprint.clone(),
                                sequence:    slot.sequence,
                            })
                        })?,
                };

                serde_json::from_value::<I::Response>(stored.response)
                    .map_err(|e| I::Error::from(ReplayError::Deserialize(e.to_string())))
            }

            // ── passthrough / off ─────────────────────────────────────────────
            ReplayMode::Passthrough | ReplayMode::Off => {
                self.interceptor.execute(req).await
            }
        }
    }
}
