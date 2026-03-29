/// Runtime integration tests for the three Phase-16 macros.
///
/// Each test uses MockContext::with_store so it carries its own InMemoryStore —
/// no global state, no inter-test interference.

use std::sync::Arc;

use replay_core::{MockContext, ReplayMode, MOCK_CTX};
use replay_macro::{instrument, instrument_spawns, instrument_trait, record_io};
use replay_store::{InMemoryStore, InteractionStore};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

fn store() -> Arc<InMemoryStore> {
    Arc::new(InMemoryStore::new())
}

// ── #[record_io] on a free async fn ──────────────────────────────────────────

#[record_io]
async fn double(x: u64) -> u64 {
    x * 2
}

#[tokio::test]
async fn record_io_records_free_fn() {
    let store     = store();
    let record_id = Uuid::new_v4();

    let ctx = MockContext::with_store(record_id, ReplayMode::Record, store.clone());
    let result = MOCK_CTX.scope(ctx, async { double(21).await }).await;
    assert_eq!(result, 42);

    let stored = store.get_by_record_id(record_id).await.unwrap();
    assert_eq!(stored.len(), 1, "interaction must be written to store");
}

#[tokio::test]
async fn record_io_replays_free_fn_from_store() {
    let store     = store();
    let record_id = Uuid::new_v4();

    // Record phase — compute with real arg
    let ctx = MockContext::with_store(record_id, ReplayMode::Record, store.clone());
    MOCK_CTX.scope(ctx, async { double(21).await }).await;

    // Replay phase — arg is 0 but stored result (42) must be returned
    let ctx = MockContext::with_store(record_id, ReplayMode::Replay, store.clone());
    let replayed = MOCK_CTX.scope(ctx, async { double(0).await }).await;
    assert_eq!(replayed, 42, "replay must return stored value, not recompute");
}

// ── #[instrument] on an impl block ───────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct SumResult(u64);

struct Adder;

#[instrument]
impl Adder {
    async fn add(&self, a: u64, b: u64) -> SumResult {
        SumResult(a + b)
    }

    // Sync — must pass through unchanged (no instrumentation)
    fn label(&self) -> &'static str {
        "adder"
    }
}

#[tokio::test]
async fn instrument_impl_records_async_method() {
    let store     = store();
    let record_id = Uuid::new_v4();
    let adder     = Adder;

    let ctx = MockContext::with_store(record_id, ReplayMode::Record, store.clone());
    let result = MOCK_CTX.scope(ctx, async { adder.add(3, 4).await }).await;
    assert_eq!(result, SumResult(7));

    let stored = store.get_by_record_id(record_id).await.unwrap();
    assert_eq!(stored.len(), 1, "async method must be recorded");
}

#[tokio::test]
async fn instrument_impl_sync_method_passes_through() {
    // Sync methods must still compile and work — no context needed
    let adder = Adder;
    assert_eq!(adder.label(), "adder");
}

#[tokio::test]
async fn instrument_impl_replays_async_method() {
    let store     = store();
    let record_id = Uuid::new_v4();
    let adder     = Adder;

    // Record
    let ctx = MockContext::with_store(record_id, ReplayMode::Record, store.clone());
    MOCK_CTX.scope(ctx, async { adder.add(3, 4).await }).await;

    // Replay — args (0, 0) differ but stored result (7) must be returned
    let ctx = MockContext::with_store(record_id, ReplayMode::Replay, store.clone());
    let result = MOCK_CTX.scope(ctx, async { adder.add(0, 0).await }).await;
    assert_eq!(result, SumResult(7), "replay must return stored result");
}

// ── #[instrument_spawns] rewrites tokio::spawn ────────────────────────────────

#[instrument_spawns]
async fn spawner() -> bool {
    // The macro rewrites this tokio::spawn → replay_core::spawn_with_ctx
    let handle = tokio::spawn(async {
        replay_core::current_ctx().is_some()
    });
    handle.await.unwrap()
}

#[tokio::test]
async fn instrument_spawns_propagates_context_into_child_task() {
    let store     = store();
    let record_id = Uuid::new_v4();

    let ctx = MockContext::with_store(record_id, ReplayMode::Record, store.clone());
    let got = MOCK_CTX.scope(ctx, async { spawner().await }).await;

    assert!(got, "spawned task must inherit MockContext thanks to spawn_with_ctx");
}

// ── #[instrument_trait] generates a wrapper struct ───────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Greeting(String);

#[instrument_trait]
pub trait Greeter {
    async fn greet(&self, name: String) -> Greeting;
}

struct RealGreeter;

impl Greeter for RealGreeter {
    async fn greet(&self, name: String) -> Greeting {
        Greeting(format!("Hello, {name}!"))
    }
}

#[tokio::test]
async fn instrument_trait_wrapper_struct_exists() {
    // Verify the generated `ReplayGreeter<T: Greeter>` compiles and delegates.
    let store     = store();
    let record_id = Uuid::new_v4();

    let wrapped = ReplayGreeter(RealGreeter);

    let ctx = MockContext::with_store(record_id, ReplayMode::Record, store.clone());
    let result = MOCK_CTX
        .scope(ctx, async { wrapped.greet("World".to_string()).await })
        .await;
    assert_eq!(result, Greeting("Hello, World!".to_string()));

    let stored = store.get_by_record_id(record_id).await.unwrap();
    assert_eq!(stored.len(), 1, "trait method call must be recorded");
}

#[tokio::test]
async fn instrument_trait_wrapper_replays_call() {
    let store     = store();
    let record_id = Uuid::new_v4();

    let wrapped = ReplayGreeter(RealGreeter);

    // Record with "Alice"
    let ctx = MockContext::with_store(record_id, ReplayMode::Record, store.clone());
    MOCK_CTX
        .scope(ctx, async { wrapped.greet("Alice".to_string()).await })
        .await;

    // Replay — arg differs but stored result must be returned
    let ctx = MockContext::with_store(record_id, ReplayMode::Replay, store.clone());
    let result = MOCK_CTX
        .scope(ctx, async { wrapped.greet("IGNORED".to_string()).await })
        .await;
    assert_eq!(result, Greeting("Hello, Alice!".to_string()));
}
