use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use uuid::Uuid;

use crate::global_store::MacroStore;
use crate::types::{CallType, ReplayMode};

/// Per-request ambient state shared by all interceptors via a task-local.
#[derive(Clone, Debug)]
pub struct MockContext {
    pub record_id:    Uuid,
    pub mode:         ReplayMode,
    pub build_hash:   String,
    pub service_name: String,
    pub tag:          String,
    /// Store scoped to this recording context. When set it takes precedence
    /// over the process-wide global store, giving test isolation for free.
    pub store:        Option<Arc<dyn MacroStore>>,
    /// In `Shadow` mode: the temporary record_id that `#[record_io]` writes
    /// new interactions to.  `None` in all other modes.
    pub capture_id:   Option<Uuid>,
    sequence:         Arc<AtomicU32>,
}

impl MockContext {
    pub fn new(mode: ReplayMode) -> Self {
        Self {
            record_id:    Uuid::new_v4(),
            mode,
            build_hash:   String::new(),
            service_name: String::new(),
            tag:          String::new(),
            store:        None,
            capture_id:   None,
            sequence:     Arc::new(AtomicU32::new(0)),
        }
    }

    pub fn with_id(record_id: Uuid, mode: ReplayMode) -> Self {
        Self {
            record_id,
            mode,
            build_hash:   String::new(),
            service_name: String::new(),
            tag:          String::new(),
            store:        None,
            capture_id:   None,
            sequence:     Arc::new(AtomicU32::new(0)),
        }
    }

    pub fn with_store(
        record_id: Uuid,
        mode:      ReplayMode,
        store:     Arc<dyn MacroStore>,
    ) -> Self {
        Self {
            record_id,
            mode,
            build_hash:   String::new(),
            service_name: String::new(),
            tag:          String::new(),
            store:        Some(store),
            capture_id:   None,
            sequence:     Arc::new(AtomicU32::new(0)),
        }
    }

    /// Constructor for Shadow mode: mocks external deps from `record_id` but
    /// writes `#[record_io]` results to `capture_id`.
    /// Falls back to the process-wide global store for the `#[record_io]` runtime.
    pub fn shadow(record_id: Uuid, capture_id: Uuid) -> Self {
        Self {
            record_id,
            mode:         ReplayMode::Shadow,
            build_hash:   String::new(),
            service_name: String::new(),
            tag:          String::new(),
            store:        None,
            capture_id:   Some(capture_id),
            sequence:     Arc::new(AtomicU32::new(0)),
        }
    }

    /// Shadow mode with an explicit store — useful in tests to avoid global state.
    pub fn shadow_with_store(record_id: Uuid, capture_id: Uuid, store: Arc<dyn MacroStore>) -> Self {
        Self {
            record_id,
            mode:         ReplayMode::Shadow,
            build_hash:   String::new(),
            service_name: String::new(),
            tag:          String::new(),
            store:        Some(store),
            capture_id:   Some(capture_id),
            sequence:     Arc::new(AtomicU32::new(0)),
        }
    }

    /// Monotonically increasing counter across all concurrent interceptors.
    pub fn next_seq(&self) -> u32 {
        self.sequence.fetch_add(1, Ordering::SeqCst)
    }

    /// The effective store: context-scoped first, global fallback.
    pub fn effective_store(&self) -> Option<Arc<dyn MacroStore>> {
        self.store.clone().or_else(crate::global_store::global_store)
    }
}

/// A lightweight slot describing one intercepted call site.
/// Created by each interceptor before it decides to record or replay.
#[derive(Debug, Clone)]
pub struct InteractionSlot {
    pub record_id:    Uuid,
    pub sequence:     u32,
    pub call_type:    CallType,
    pub fingerprint:  String,
    pub mode:         ReplayMode,
    pub build_hash:   String,
    pub service_name: String,
    pub tag:          String,
    /// Effective store for this slot (already resolved from context).
    pub store:        Option<Arc<dyn MacroStore>>,
    /// In `Shadow` mode: the capture record_id that `#[record_io]` writes to.
    pub capture_id:   Option<Uuid>,
}

tokio::task_local! {
    pub static MOCK_CTX: MockContext;
}

/// Read the current context without consuming it.
pub fn current_ctx() -> Option<MockContext> {
    MOCK_CTX.try_with(|c| c.clone()).ok()
}

/// Allocate the next interaction slot from the ambient context.
/// Returns None when no context is active.
pub fn next_interaction_slot(call_type: CallType, fingerprint: String) -> Option<InteractionSlot> {
    MOCK_CTX.try_with(|ctx| InteractionSlot {
        record_id:    ctx.record_id,
        sequence:     ctx.next_seq(),
        call_type,
        fingerprint,
        mode:         ctx.mode.clone(),
        build_hash:   ctx.build_hash.clone(),
        service_name: ctx.service_name.clone(),
        tag:          ctx.tag.clone(),
        store:        ctx.effective_store(),
        capture_id:   ctx.capture_id,
    }).ok()
}

/// Run a future inside a fresh recording context (uses global store).
pub async fn with_recording<F, T>(mode: ReplayMode, f: F) -> T
where
    F: Future<Output = T>,
{
    let ctx = MockContext::new(mode);
    MOCK_CTX.scope(ctx, f).await
}

/// Run a future with a specific record_id (uses global store).
pub async fn with_recording_id<F, T>(record_id: Uuid, mode: ReplayMode, f: F) -> T
where
    F: Future<Output = T>,
{
    let ctx = MockContext::with_id(record_id, mode);
    MOCK_CTX.scope(ctx, f).await
}

/// Run a future with a specific record_id **and** a scoped store.
/// The scoped store takes precedence over the global store for this context.
/// Use this in tests for full isolation.
pub async fn with_recording_store<F, T>(
    record_id: Uuid,
    mode:      ReplayMode,
    store:     Arc<dyn MacroStore>,
    f:         F,
) -> T
where
    F: Future<Output = T>,
{
    let ctx = MockContext::with_store(record_id, mode, store);
    MOCK_CTX.scope(ctx, f).await
}

/// Drop-in replacement for `tokio::spawn` that propagates the recording context.
pub fn spawn_with_ctx<F>(fut: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match MOCK_CTX.try_with(|ctx| ctx.clone()) {
        Ok(ctx) => tokio::spawn(MOCK_CTX.scope(ctx, fut)),
        Err(_)  => tokio::spawn(fut),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sequence_is_monotonic() {
        with_recording(ReplayMode::Record, async {
            let s1 = next_interaction_slot(CallType::Http, "a".into()).unwrap();
            let s2 = next_interaction_slot(CallType::Postgres, "b".into()).unwrap();
            let s3 = next_interaction_slot(CallType::Redis, "c".into()).unwrap();
            assert_eq!(s1.sequence, 0);
            assert_eq!(s2.sequence, 1);
            assert_eq!(s3.sequence, 2);
        })
        .await;
    }

    #[tokio::test]
    async fn no_slot_outside_context() {
        let slot = next_interaction_slot(CallType::Http, "test".into());
        assert!(slot.is_none());
    }

    #[tokio::test]
    async fn context_propagates_through_spawn() {
        with_recording(ReplayMode::Record, async {
            let parent_id = current_ctx().unwrap().record_id;
            let handle = spawn_with_ctx(async {
                current_ctx().map(|c| c.record_id)
            });
            let spawned_id = handle.await.unwrap();
            assert_eq!(Some(parent_id), spawned_id);
        })
        .await;
    }

    #[tokio::test]
    async fn sequence_atomic_across_concurrent_slots() {
        with_recording(ReplayMode::Record, async {
            let (s1, s2, s3) = tokio::join!(
                async { next_interaction_slot(CallType::Http, "x".into()).unwrap().sequence },
                async { next_interaction_slot(CallType::Http, "y".into()).unwrap().sequence },
                async { next_interaction_slot(CallType::Http, "z".into()).unwrap().sequence },
            );
            let mut seqs = [s1, s2, s3];
            seqs.sort_unstable();
            assert_eq!(seqs, [0, 1, 2]);
        })
        .await;
    }

    #[tokio::test]
    async fn slot_carries_tag_and_service_name() {
        let mut ctx = MockContext::new(ReplayMode::Record);
        ctx.tag = "my-tag".into();
        ctx.service_name = "my-svc".into();
        MOCK_CTX.scope(ctx, async {
            let slot = next_interaction_slot(CallType::Http, "GET /foo".into()).unwrap();
            assert_eq!(slot.tag, "my-tag");
            assert_eq!(slot.service_name, "my-svc");
        }).await;
    }
}
