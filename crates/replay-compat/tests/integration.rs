/// Integration tests for replay-compat.
///
/// Each test uses an InMemoryStore and a wiremock server so no external
/// services are needed. SQL and Redis tests are in separate feature-gated
/// modules that require real service connections.

use std::sync::Arc;

use replay_compat::http;
use replay_core::{CallType, InteractionStore, MockContext, ReplayMode, MOCK_CTX};
use replay_store::InMemoryStore;
use serde_json::json;
use uuid::Uuid;
use wiremock::{matchers::{method, path}, Mock, MockServer, ResponseTemplate};

// ── HTTP record + replay ──────────────────────────────────────────────────────

#[tokio::test]
async fn http_client_records_interaction_into_store() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/ping"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "pong": true })),
        )
        .mount(&mock_server)
        .await;

    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let client    = http::Client::with_store(store.clone());

    let ctx = MockContext::with_id(record_id, ReplayMode::Record);
    MOCK_CTX
        .scope(ctx, async {
            let resp = client
                .get(format!("{}/api/v1/ping", mock_server.uri()))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), 200);
        })
        .await;

    let stored = store.get_by_record_id(record_id).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].call_type, CallType::Http);
    assert_eq!(stored[0].sequence, 0);
}

#[tokio::test]
async fn http_client_replays_without_hitting_server() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/data"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "result": "live" })),
        )
        .expect(1) // Only one real request — during record phase
        .mount(&mock_server)
        .await;

    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let client    = http::Client::with_store(store.clone());
    let url       = format!("{}/api/v1/data", mock_server.uri());

    // ── record phase ─────────────────────────────────────────────────────────
    let ctx = MockContext::with_id(record_id, ReplayMode::Record);
    MOCK_CTX
        .scope(ctx, async {
            let resp = client.get(&url).send().await.unwrap();
            assert_eq!(resp.status(), 200);
        })
        .await;

    // ── replay phase — server must NOT be queried ─────────────────────────────
    let ctx = MockContext::with_id(record_id, ReplayMode::Replay);
    MOCK_CTX
        .scope(ctx, async {
            let resp = client.get(&url).send().await.unwrap();
            assert_eq!(resp.status(), 200);
        })
        .await;

    // wiremock verifies the expectation (exactly 1 request) on drop
    mock_server.verify().await;
}

// ── multiple interactions in one request ─────────────────────────────────────

#[tokio::test]
async fn multiple_outbound_calls_get_sequential_sequence_numbers() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
        .mount(&mock_server)
        .await;

    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let client    = http::Client::with_store(store.clone());

    let ctx = MockContext::with_id(record_id, ReplayMode::Record);
    MOCK_CTX
        .scope(ctx, async {
            client.get(format!("{}/a", mock_server.uri())).send().await.unwrap();
            client.get(format!("{}/b", mock_server.uri())).send().await.unwrap();
            client.get(format!("{}/c", mock_server.uri())).send().await.unwrap();
        })
        .await;

    let stored = store.get_by_record_id(record_id).await.unwrap();
    assert_eq!(stored.len(), 3);
    // Each call must get a unique, monotonically increasing sequence number.
    let mut seqs: Vec<u32> = stored.iter().map(|i| i.sequence).collect();
    seqs.sort_unstable();
    assert_eq!(seqs, vec![0, 1, 2]);
}

// ── tokio::spawn context propagation ─────────────────────────────────────────

#[tokio::test]
async fn spawn_propagates_mock_context_into_child_task() {
    use replay_core::current_ctx;
    use replay_compat::tokio::task::spawn;

    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();

    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let ctx = MockContext::with_id(record_id, ReplayMode::Record);
    MOCK_CTX
        .scope(ctx, async {
            // Spawn a child task — it should inherit the MockContext
            let url = format!("{}/spawn-test", mock_server.uri());
            let client_clone = http::Client::with_store(store.clone());
            let handle = spawn(async move {
                let ctx = current_ctx();
                // Context should be present in the spawned task
                assert!(ctx.is_some(), "MockContext must propagate through spawn");
                assert_eq!(ctx.unwrap().record_id, record_id);
                client_clone.get(&url).send().await.unwrap();
            });
            handle.await.unwrap();
        })
        .await;

    let stored = store.get_by_record_id(record_id).await.unwrap();
    assert_eq!(stored.len(), 1, "interaction recorded from spawned task");
}

// ── install() / global store ──────────────────────────────────────────────────

#[tokio::test]
async fn install_allows_client_new_to_work() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&mock_server)
        .await;

    let store = Arc::new(InMemoryStore::new());
    replay_compat::install(store.clone(), ReplayMode::Record);

    let client    = http::Client::new(); // uses global store from install()
    let record_id = Uuid::new_v4();

    let ctx = MockContext::with_id(record_id, ReplayMode::Record);
    MOCK_CTX
        .scope(ctx, async {
            let resp = client
                .get(format!("{}/global-test", mock_server.uri()))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), 204);
        })
        .await;

    let stored = store.get_by_record_id(record_id).await.unwrap();
    assert_eq!(stored.len(), 1);
}
