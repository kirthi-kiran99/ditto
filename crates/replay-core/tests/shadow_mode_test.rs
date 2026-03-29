/// Tests for Shadow mode — verifies that #[record_io] functions execute their
/// real code in Shadow mode and write results to capture_id, while external
/// interceptors (via InterceptorRunner) still mock from the original recording.

use std::sync::{Arc, atomic::{AtomicU32, Ordering}};

use async_trait::async_trait;
use replay_core::{
    CallType, Interaction, InteractionStore, InterceptorRunner,
    MockContext, ReplayError, ReplayInterceptor, ReplayMode,
    context::{MOCK_CTX, next_interaction_slot},
    global_store::{set_global_store, record_fn_call},
};
use replay_store::InMemoryStore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

// ── helper: simple interceptor that tracks real calls ─────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct MockResp { value: String }

#[derive(Debug, thiserror::Error)]
enum TestError { #[error(transparent)] Replay(#[from] ReplayError) }

struct CountInterceptor { calls: Arc<AtomicU32> }

#[async_trait]
impl ReplayInterceptor for CountInterceptor {
    type Request  = String;
    type Response = MockResp;
    type Error    = TestError;
    fn call_type(&self) -> CallType { CallType::Http }
    fn fingerprint(&self, req: &String) -> String { format!("GET /{req}") }
    fn normalize_request(&self, req: &String) -> Value { json!({ "req": req }) }
    fn normalize_response(&self, resp: &MockResp) -> Value { json!({ "value": resp.value }) }
    async fn execute(&self, req: String) -> Result<MockResp, TestError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(MockResp { value: format!("live:{req}") })
    }
}

fn build_runner(interceptor: CountInterceptor, store: Arc<dyn InteractionStore>)
    -> InterceptorRunner<CountInterceptor>
{
    InterceptorRunner::new(interceptor, store)
}

// ── Test 1: Shadow mode propagates capture_id to InteractionSlot ──────────────

#[tokio::test]
async fn shadow_context_propagates_capture_id() {
    let record_id  = Uuid::new_v4();
    let capture_id = Uuid::new_v4();
    let ctx = MockContext::shadow(record_id, capture_id);

    MOCK_CTX.scope(ctx, async {
        let slot = next_interaction_slot(CallType::Function, "fn::test".into()).unwrap();
        assert_eq!(slot.mode, ReplayMode::Shadow);
        assert_eq!(slot.record_id, record_id);
        assert_eq!(slot.capture_id, Some(capture_id));
    }).await;
}

// ── Test 2: record_fn_call writes to capture_id in Shadow mode ────────────────

#[tokio::test]
async fn record_fn_call_writes_to_capture_id_in_shadow_mode() {
    let store = Arc::new(InMemoryStore::new());

    let record_id  = Uuid::new_v4();
    let capture_id = Uuid::new_v4();
    // Use shadow_with_store for test isolation — no global state touched.
    let ctx = MockContext::shadow_with_store(
        record_id,
        capture_id,
        store.clone() as Arc<dyn replay_core::MacroStore>,
    );

    MOCK_CTX.scope(ctx, async {
        let slot = next_interaction_slot(CallType::Function, "compute_tax".into()).unwrap();
        assert_eq!(slot.mode, ReplayMode::Shadow);

        // Simulate what the #[record_io] macro does: call record_fn_call
        record_fn_call(slot, json!({ "amount": 99.99 }), json!(9.0_f64), 1).await;
    }).await;

    // Interaction must appear under capture_id, NOT record_id
    let in_capture = store.get_by_record_id(capture_id).await.unwrap();
    let in_original = store.get_by_record_id(record_id).await.unwrap();

    assert_eq!(in_capture.len(), 1, "should have 1 interaction under capture_id");
    assert_eq!(in_original.len(), 0, "original recording must not be polluted");

    let cap = &in_capture[0];
    assert_eq!(cap.fingerprint, "compute_tax");
    assert_eq!(cap.response, json!(9.0_f64));
    assert_eq!(cap.call_type, CallType::Function);
}

// ── Test 3: InterceptorRunner in Shadow mode mocks from original recording ────

#[tokio::test]
async fn interceptor_runner_shadow_mocks_from_original_not_capture() {
    let store = Arc::new(InMemoryStore::new());

    // Seed the store with a recorded interaction under record_id
    let record_id  = Uuid::new_v4();
    let capture_id = Uuid::new_v4();
    store.seed([Interaction::new(
        record_id, 1, CallType::Http, "GET /widget".into(),
        json!({ "req": "widget" }),
        json!({ "value": "stored:widget" }),
        10,
    )]);

    let calls = Arc::new(AtomicU32::new(0));
    let runner = build_runner(
        CountInterceptor { calls: calls.clone() },
        store.clone() as Arc<dyn InteractionStore>,
    );

    let ctx = MockContext::shadow(record_id, capture_id);

    let result = MOCK_CTX.scope(ctx, async {
        // seq=0 is consumed first (simulate middleware claiming it)
        let _seq0 = next_interaction_slot(CallType::Http, "entry".into());
        // seq=1 is what our interceptor will use
        runner.run("widget".to_string()).await
    }).await.unwrap();

    // Must return STORED value, not execute real client
    assert_eq!(result.value, "stored:widget",
        "Shadow mode must mock external deps from the original recording");
    assert_eq!(calls.load(Ordering::SeqCst), 0,
        "execute() must NOT be called in Shadow mode");
}

// ── Test 4: InMemoryStore delete_by_record_id removes only the target ─────────

#[tokio::test]
async fn delete_by_record_id_only_removes_target() {
    let store     = Arc::new(InMemoryStore::new());
    let keep_id   = Uuid::new_v4();
    let delete_id = Uuid::new_v4();

    store.seed([
        Interaction::new(keep_id,   0, CallType::Http, "GET /a".into(), json!({}), json!({}), 1),
        Interaction::new(delete_id, 0, CallType::Http, "GET /b".into(), json!({}), json!({}), 1),
    ]);

    store.delete_by_record_id(delete_id).await.unwrap();

    let after_keep   = store.get_by_record_id(keep_id).await.unwrap();
    let after_delete = store.get_by_record_id(delete_id).await.unwrap();

    assert_eq!(after_keep.len(),   1, "unrelated recording must not be deleted");
    assert_eq!(after_delete.len(), 0, "target recording must be deleted");
}

// ── Test 5: Full shadow round-trip — record then shadow then compare ───────────

#[tokio::test]
async fn shadow_roundtrip_shows_different_fn_result() {
    let store = Arc::new(InMemoryStore::new());
    set_global_store(store.clone() as Arc<dyn replay_core::MacroStore>);

    let record_id = Uuid::new_v4();

    // Phase 1: "Record" — store the correct tax value (8%)
    {
        let ctx = MockContext::with_id(record_id, ReplayMode::Record);
        MOCK_CTX.scope(ctx, async {
            let slot = next_interaction_slot(CallType::Function, "compute_tax".into()).unwrap();
            // Correct implementation: 8% tax on $100 = $8
            record_fn_call(slot, json!({ "amount": 100.0 }), json!(8.0_f64), 1).await;
        }).await;
    }

    let recorded = store.get_by_record_id(record_id).await.unwrap();
    assert_eq!(recorded[0].response, json!(8.0_f64), "recorded value must be 8.0");

    // Phase 2: "Shadow replay" — faulty implementation computes 9% tax
    let capture_id = Uuid::new_v4();
    {
        let ctx = MockContext::shadow(record_id, capture_id);
        MOCK_CTX.scope(ctx, async {
            let slot = next_interaction_slot(CallType::Function, "compute_tax".into()).unwrap();
            // Faulty implementation: 9% tax on $100 = $9
            record_fn_call(slot, json!({ "amount": 100.0 }), json!(9.0_f64), 1).await;
        }).await;
    }

    let captured = store.get_by_record_id(capture_id).await.unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].response, json!(9.0_f64), "captured value must be 9.0 (faulty)");

    // Phase 3: verify the diff would catch the regression
    // recorded = 8.0, captured = 9.0 → these differ
    assert_ne!(
        recorded[0].response, captured[0].response,
        "diff engine would detect this as a regression"
    );

    // Phase 4: cleanup (as harness does after diffing)
    store.delete_by_record_id(capture_id).await.unwrap();
    let after = store.get_by_record_id(capture_id).await.unwrap();
    assert!(after.is_empty(), "temp capture record must be cleaned up");
}
