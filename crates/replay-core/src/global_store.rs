use std::sync::{Arc, OnceLock, RwLock};

use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

use crate::interaction::Interaction;

/// Minimal store interface needed by the `#[record_io]` macro at runtime.
/// Both `InMemoryStore` and `PostgresStore` implement this.
///
/// Kept separate from the full `InteractionStore` trait so `replay-core`
/// doesn't need to depend on `replay-store`.
#[async_trait]
pub trait MacroStore: Send + Sync + std::fmt::Debug {
    /// Persist a function-call interaction (fire-and-forget).
    async fn store_fn_call(&self, interaction: &Interaction);

    /// Look up the stored response for a function call.
    /// Returns the `response` JSON Value directly (caller deserializes to return type).
    /// Falls back to fuzzy (nearest-sequence) matching automatically.
    async fn load_fn_response(
        &self,
        record_id:   Uuid,
        fingerprint: &str,
        sequence:    u32,
    ) -> Option<Value>;
}

// RwLock allows replacement, which is needed for test isolation.
// In production `set_global_store` is called once at startup.
static GLOBAL: OnceLock<RwLock<Option<Arc<dyn MacroStore>>>> = OnceLock::new();

fn global_lock() -> &'static RwLock<Option<Arc<dyn MacroStore>>> {
    GLOBAL.get_or_init(|| RwLock::new(None))
}

/// Register the store used by `#[record_io]` at runtime.
/// Safe to call multiple times — replaces the previous store.
/// In production: call once at application startup.
/// In tests: call at the start of each test to get a fresh isolated store.
pub fn set_global_store(store: Arc<dyn MacroStore>) {
    *global_lock().write().expect("global store lock poisoned") = Some(store);
}

/// Access the global store. Returns `None` if `set_global_store` was never called.
pub fn global_store() -> Option<Arc<dyn MacroStore>> {
    global_lock().read().expect("global store lock poisoned").clone()
}

// ── runtime helpers called by macro-generated code ────────────────────────────

use crate::{context::InteractionSlot, types::CallStatus};

/// Record the result of a function call.
/// Uses the store from the slot (context-scoped), falling back to global.
pub async fn record_fn_call(
    slot:        InteractionSlot,
    request:     Value,
    response:    Value,
    duration_ms: u64,
) {
    // Slot already carries the effective store resolved at slot-creation time
    let store = match slot.store.clone().or_else(global_store) {
        Some(s) => s,
        None    => return,
    };

    // In Shadow mode, write to the capture_id so the harness can read new values
    // without polluting the original recording.
    let write_record_id = if matches!(slot.mode, crate::types::ReplayMode::Shadow) {
        slot.capture_id.unwrap_or(slot.record_id)
    } else {
        slot.record_id
    };

    let interaction = Interaction {
        id:           uuid::Uuid::new_v4(),
        record_id:    write_record_id,
        parent_id:    None,
        sequence:     slot.sequence,
        call_type:    slot.call_type,
        fingerprint:  slot.fingerprint,
        request,
        response,
        duration_ms,
        status:       CallStatus::Completed,
        error:        None,
        recorded_at:  chrono::Utc::now(),
        build_hash:   slot.build_hash,
        service_name: slot.service_name,
        tag:          slot.tag,
    };

    store.store_fn_call(&interaction).await;
}

/// Retrieve the stored return-value JSON for a function call during replay.
pub async fn replay_fn_call(slot: &InteractionSlot) -> Option<Value> {
    let store = slot.store.clone().or_else(global_store)?;
    store
        .load_fn_response(slot.record_id, &slot.fingerprint, slot.sequence)
        .await
}
