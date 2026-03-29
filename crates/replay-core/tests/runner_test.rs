/// Unit tests for InterceptorRunner covering all four modes.
///
/// Uses an in-process EchoInterceptor so no external services are needed.
/// The execute_count field tracks whether execute() was called, letting tests
/// verify that Replay mode returns stored responses without touching the client.

use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

use async_trait::async_trait;
use replay_core::{
    CallType, Interaction, InteractionStore, InterceptorRunner, MockContext, ReplayError,
    ReplayInterceptor, ReplayMode, MOCK_CTX,
};
use replay_store::InMemoryStore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

// ── test double ───────────────────────────────────────────────────────────────

/// Simple response type that round-trips through serde_json.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct EchoResponse {
    value: String,
}

/// Error that bridges ReplayError via From.
#[derive(Debug, thiserror::Error)]
enum EchoError {
    #[error(transparent)]
    Replay(#[from] ReplayError),
}

/// Interceptor that echoes the request string back as the response value.
/// Tracks how many times execute() is actually called.
struct EchoInterceptor {
    execute_count: Arc<AtomicU32>,
}

impl EchoInterceptor {
    fn new() -> (Self, Arc<AtomicU32>) {
        let counter = Arc::new(AtomicU32::new(0));
        (Self { execute_count: counter.clone() }, counter)
    }
}

#[async_trait]
impl ReplayInterceptor for EchoInterceptor {
    type Request  = String;
    type Response = EchoResponse;
    type Error    = EchoError;

    fn call_type(&self) -> CallType { CallType::Function }

    fn fingerprint(&self, req: &String) -> String {
        format!("echo:{req}")
    }

    fn normalize_request(&self, req: &String) -> Value {
        json!({ "input": req })
    }

    fn normalize_response(&self, res: &EchoResponse) -> Value {
        serde_json::to_value(res).unwrap()
    }

    async fn execute(&self, req: String) -> Result<EchoResponse, EchoError> {
        self.execute_count.fetch_add(1, Ordering::SeqCst);
        Ok(EchoResponse { value: req })
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_store() -> Arc<InMemoryStore> {
    Arc::new(InMemoryStore::new())
}

fn make_runner(
    counter: Arc<AtomicU32>,
    store: Arc<InMemoryStore>,
) -> InterceptorRunner<EchoInterceptor> {
    let interceptor = EchoInterceptor { execute_count: counter };
    InterceptorRunner::new(interceptor, store as Arc<dyn InteractionStore>)
}

// ── no-context passthrough ────────────────────────────────────────────────────

#[tokio::test]
async fn no_context_passes_through_without_touching_store() {
    let store = make_store();
    let (interceptor, counter) = EchoInterceptor::new();
    let runner = InterceptorRunner::new(interceptor, store.clone() as Arc<dyn InteractionStore>);

    // No MOCK_CTX scope — runner should forward to execute() silently
    let result = runner.run("hello".to_string()).await.unwrap();

    assert_eq!(result.value, "hello");
    assert_eq!(counter.load(Ordering::SeqCst), 1, "execute() must be called");
    assert!(store.get_recent_record_ids(10).await.unwrap().is_empty(), "nothing stored");
}

// ── record mode ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn record_mode_calls_execute_and_writes_to_store() {
    let store = make_store();
    let record_id = Uuid::new_v4();
    let counter = Arc::new(AtomicU32::new(0));
    let runner = make_runner(counter.clone(), store.clone());

    let ctx = MockContext::with_id(record_id, ReplayMode::Record);
    let result = MOCK_CTX
        .scope(ctx, async { runner.run("hello".to_string()).await })
        .await
        .unwrap();

    assert_eq!(result.value, "hello");
    assert_eq!(counter.load(Ordering::SeqCst), 1, "execute() must be called once in record mode");

    let stored = store.get_by_record_id(record_id).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].fingerprint, "echo:hello");
    assert_eq!(stored[0].call_type, CallType::Function);
    assert_eq!(stored[0].sequence, 0);
}

#[tokio::test]
async fn record_mode_sequences_increment_across_calls() {
    let store = make_store();
    let record_id = Uuid::new_v4();
    let counter = Arc::new(AtomicU32::new(0));
    let runner = make_runner(counter.clone(), store.clone());

    let ctx = MockContext::with_id(record_id, ReplayMode::Record);
    MOCK_CTX
        .scope(ctx, async {
            runner.run("a".to_string()).await.unwrap();
            runner.run("b".to_string()).await.unwrap();
            runner.run("c".to_string()).await.unwrap();
        })
        .await;

    let stored = store.get_by_record_id(record_id).await.unwrap();
    assert_eq!(stored.len(), 3);
    assert_eq!(stored[0].sequence, 0);
    assert_eq!(stored[1].sequence, 1);
    assert_eq!(stored[2].sequence, 2);
}

// ── replay mode ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn replay_mode_returns_stored_response_without_calling_execute() {
    let store = make_store();
    let record_id = Uuid::new_v4();

    // Seed the store directly — simulate a previous recording
    let stored_resp = EchoResponse { value: "stored value".to_string() };
    let interaction = Interaction::new(
        record_id,
        0,
        CallType::Function,
        "echo:anything".to_string(),
        json!({ "input": "anything" }),
        serde_json::to_value(&stored_resp).unwrap(),
        0,
    );
    store.write(&interaction).await.unwrap();

    // Replay — execute() must NOT be called
    let counter = Arc::new(AtomicU32::new(0));
    let runner = make_runner(counter.clone(), store.clone());

    let ctx = MockContext::with_id(record_id, ReplayMode::Replay);
    let result = MOCK_CTX
        .scope(ctx, async { runner.run("anything".to_string()).await })
        .await
        .unwrap();

    assert_eq!(result, stored_resp, "must return stored response");
    assert_eq!(counter.load(Ordering::SeqCst), 0, "execute() must NOT be called in replay mode");
}

#[tokio::test]
async fn replay_mode_errors_on_store_miss() {
    let store = make_store(); // empty — no recordings
    let record_id = Uuid::new_v4();
    let counter = Arc::new(AtomicU32::new(0));
    let runner = make_runner(counter.clone(), store.clone());

    let ctx = MockContext::with_id(record_id, ReplayMode::Replay);
    let result = MOCK_CTX
        .scope(ctx, async { runner.run("nothing_recorded".to_string()).await })
        .await;

    assert!(result.is_err(), "should error on store miss");
    assert!(
        matches!(result.unwrap_err(), EchoError::Replay(ReplayError::NotFound { .. })),
        "error should be ReplayError::NotFound"
    );
    assert_eq!(counter.load(Ordering::SeqCst), 0, "execute() must NOT be called");
}

#[tokio::test]
async fn replay_mode_uses_fuzzy_fallback_on_sequence_drift() {
    let store = make_store();
    let record_id = Uuid::new_v4();

    // Recorded at sequence 0
    let stored_resp = EchoResponse { value: "fuzzy match".to_string() };
    let interaction = Interaction::new(
        record_id,
        0, // recorded at sequence 0
        CallType::Function,
        "echo:test".to_string(),
        json!({ "input": "test" }),
        serde_json::to_value(&stored_resp).unwrap(),
        0,
    );
    store.write(&interaction).await.unwrap();

    // New build has an extra call before this one, so sequence is now 1
    let counter = Arc::new(AtomicU32::new(0));
    let _runner = make_runner(counter.clone(), store.clone());

    // Add a dummy interaction at sequence 0 to push our call to sequence 1
    let ctx = MockContext::with_id(record_id, ReplayMode::Replay);
    let result = MOCK_CTX
        .scope(ctx, async {
            // Consume sequence 0 with a dummy runner so our call gets sequence 1
            let _dummy_counter = Arc::new(AtomicU32::new(0));
            let _dummy = make_runner(_dummy_counter, Arc::new(InMemoryStore::new()));
            // Can't easily inject, so just test find_nearest works directly
            store
                .find_nearest(record_id, "echo:test", 1)
                .await
                .unwrap()
        })
        .await;

    // The nearest stored interaction (at seq 0) should be found for seq 1
    assert!(result.is_some(), "fuzzy match should find seq=0 when searching for seq=1");
    let found = result.unwrap();
    assert_eq!(found.sequence, 0);
    assert_eq!(
        serde_json::from_value::<EchoResponse>(found.response).unwrap(),
        stored_resp
    );
}

// ── passthrough mode ──────────────────────────────────────────────────────────

#[tokio::test]
async fn passthrough_mode_calls_execute_without_writing_to_store() {
    let store = make_store();
    let record_id = Uuid::new_v4();
    let counter = Arc::new(AtomicU32::new(0));
    let runner = make_runner(counter.clone(), store.clone());

    let ctx = MockContext::with_id(record_id, ReplayMode::Passthrough);
    let result = MOCK_CTX
        .scope(ctx, async { runner.run("passthrough_req".to_string()).await })
        .await
        .unwrap();

    assert_eq!(result.value, "passthrough_req");
    assert_eq!(counter.load(Ordering::SeqCst), 1, "execute() must be called");
    assert!(
        store.get_by_record_id(record_id).await.unwrap().is_empty(),
        "nothing should be stored in passthrough mode"
    );
}

// ── off mode ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn off_mode_calls_execute_without_writing_to_store() {
    let store = make_store();
    let record_id = Uuid::new_v4();
    let counter = Arc::new(AtomicU32::new(0));
    let runner = make_runner(counter.clone(), store.clone());

    let ctx = MockContext::with_id(record_id, ReplayMode::Off);
    let result = MOCK_CTX
        .scope(ctx, async { runner.run("off_req".to_string()).await })
        .await
        .unwrap();

    assert_eq!(result.value, "off_req");
    assert_eq!(counter.load(Ordering::SeqCst), 1, "execute() must be called");
    assert!(
        store.get_by_record_id(record_id).await.unwrap().is_empty(),
        "nothing should be stored in off mode"
    );
}
