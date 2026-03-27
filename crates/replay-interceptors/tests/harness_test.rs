/// Replay harness tests using InMemoryStore + wiremock.
///
/// These tests verify the harness logic end-to-end:
/// - It fires the recorded entry-point at the target
/// - Diffs the outer response against the recording
/// - Reports pass/fail correctly
///
/// A wiremock server plays the role of "service under test".

use std::sync::Arc;

use replay_core::{CallType, Interaction, InteractionStore};
use replay_interceptors::harness::{HarnessConfig, HarnessStatus, ReplayHarness};
use replay_store::InMemoryStore;
use serde_json::json;
use uuid::Uuid;
use wiremock::{matchers::{header, method, path}, Mock, MockServer, ResponseTemplate};

// ── helpers ───────────────────────────────────────────────────────────────────

/// Build a minimal entry-point interaction (sequence=0, call_type=Http).
fn make_entry_point(record_id: Uuid, req_path: &str, resp_status: u16) -> Interaction {
    Interaction::new(
        record_id, 0, CallType::Http,
        format!("GET {}", req_path),
        json!({ "method": "GET", "path": req_path }),
        json!({ "status": resp_status }),
        10,
    )
}

// ── basic pass/fail tests ─────────────────────────────────────────────────────

#[tokio::test]
async fn harness_passes_when_response_matches() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/payments/pay_123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "ok"})))
        .mount(&mock_server)
        .await;

    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();

    // Pre-populate the store with a recording
    store.write(&make_entry_point(record_id, "/payments/pay_123", 200)).await.unwrap();

    let harness = ReplayHarness::new(store, mock_server.uri());
    let result  = harness.run_one(record_id).await;

    // Status 200 matches recorded 200 → pass
    assert_eq!(result.status, HarnessStatus::Passed, "expected Passed");
    assert!(!result.report.is_regression);
}

#[tokio::test]
async fn harness_fails_when_status_regresses() {
    let mock_server = MockServer::start().await;

    // Service now returns 500 instead of the recorded 200
    Mock::given(method("GET"))
        .and(path("/payments/pay_456"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({"error": "internal"})))
        .mount(&mock_server)
        .await;

    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();

    store.write(&make_entry_point(record_id, "/payments/pay_456", 200)).await.unwrap();

    let harness = ReplayHarness::new(store, mock_server.uri());
    let result  = harness.run_one(record_id).await;

    assert_eq!(result.status, HarnessStatus::Failed, "expected Failed — status regressed");
    assert!(result.report.is_regression);
}

#[tokio::test]
async fn harness_errors_when_no_recording() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4(); // no interactions in store

    let harness = ReplayHarness::new(store, "http://localhost:1".into());
    let result  = harness.run_one(record_id).await;

    assert!(matches!(result.status, HarnessStatus::Error(_)));
}

#[tokio::test]
async fn harness_errors_when_service_unreachable() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();

    store.write(&make_entry_point(record_id, "/health", 200)).await.unwrap();

    // Port 1 is unreachable
    let harness = ReplayHarness::new(store, "http://127.0.0.1:1".into());
    let result  = harness.run_one(record_id).await;

    assert!(matches!(result.status, HarnessStatus::Error(_)));
}

#[tokio::test]
async fn harness_run_all_returns_all_results() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let store  = Arc::new(InMemoryStore::new());
    let ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();

    for &id in &ids {
        store.write(&make_entry_point(id, "/health", 200)).await.unwrap();
    }

    let harness = ReplayHarness::new(store, mock_server.uri());
    let results = harness.run_all(ids.clone()).await;

    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|r| r.status == HarnessStatus::Passed));
}

// ── token translation ─────────────────────────────────────────────────────────

#[tokio::test]
async fn harness_translates_auth_tokens() {
    let mock_server = MockServer::start().await;

    // Server expects the *test* token, not the production token
    Mock::given(method("GET"))
        .and(path("/secure"))
        .and(header("authorization", "Bearer test_token_xyz"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&mock_server)
        .await;

    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();

    // Recorded with production token
    let entry = Interaction::new(
        record_id, 0, CallType::Http,
        "GET /secure".to_string(),
        json!({
            "method": "GET",
            "path": "/secure",
            "headers": { "authorization": "Bearer prod_token_abc" }
        }),
        json!({ "status": 200 }),
        5,
    );
    store.write(&entry).await.unwrap();

    let mut token_map = std::collections::HashMap::new();
    token_map.insert("prod_token_abc".to_string(), "test_token_xyz".to_string());

    let config  = HarnessConfig { token_map, ..Default::default() };
    let harness = ReplayHarness::with_config(store, mock_server.uri(), config);
    let result  = harness.run_one(record_id).await;

    assert_eq!(result.status, HarnessStatus::Passed, "translated token should match");
}

// ── x-replay-record-id header ─────────────────────────────────────────────────

#[tokio::test]
async fn harness_sends_replay_record_id_header() {
    let mock_server = MockServer::start().await;

    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();

    // Verify the harness sends x-replay-record-id
    Mock::given(method("GET"))
        .and(path("/ping"))
        .and(header("x-replay-record-id", record_id.to_string().as_str()))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    store.write(&make_entry_point(record_id, "/ping", 200)).await.unwrap();

    let harness = ReplayHarness::new(store, mock_server.uri());
    let result  = harness.run_one(record_id).await;

    // If the header wasn't sent, wiremock would return 404 (not 200), causing a failure
    assert_eq!(result.status, HarnessStatus::Passed);
}

// ── server middleware entry-point recording ───────────────────────────────────

#[tokio::test]
async fn recording_middleware_stores_entry_point_at_sequence_zero() {
    use replay_core::{context::with_recording_store, ReplayMode};
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();

    // Simulate what recording_middleware_with_store does:
    // 1. Establish context
    // 2. Claim sequence=0 for entry point
    // 3. Outgoing HTTP clients get sequences 1+

    with_recording_store(record_id, ReplayMode::Record, store.clone(), async {
        let ep_slot = replay_core::next_interaction_slot(
            CallType::Http, "GET /api/payments".to_string()
        ).unwrap();
        assert_eq!(ep_slot.sequence, 0, "entry-point must be sequence=0");

        // Subsequent interceptors get 1, 2...
        let s1 = replay_core::next_interaction_slot(CallType::Http, "GET /stripe".into()).unwrap();
        let s2 = replay_core::next_interaction_slot(CallType::Postgres, "SELECT ?".into()).unwrap();
        assert_eq!(s1.sequence, 1);
        assert_eq!(s2.sequence, 2);
    }).await;
}
