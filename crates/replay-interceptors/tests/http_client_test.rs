use std::sync::Arc;

use replay_core::{with_recording, CallType, ReplayMode};
use replay_interceptors::http_client::make_http_client;
use replay_store::{InMemoryStore, InteractionStore};
use serde_json::json;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

// ── helper ────────────────────────────────────────────────────────────────────

async fn setup() -> (MockServer, Arc<InMemoryStore>) {
    let server = MockServer::start().await;
    let store = Arc::new(InMemoryStore::new());
    (server, store)
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn records_http_call_and_stores_interaction() {
    let (server, store) = setup().await;

    Mock::given(method("GET"))
        .and(path("/payments/pay_123"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "pay_123", "status": "succeeded"})),
        )
        .mount(&server)
        .await;

    let client = make_http_client(store.clone());

    with_recording(ReplayMode::Record, async {
        let resp = client
            .get(format!("{}/payments/pay_123", server.uri()))
            .send()
            .await
            .expect("request failed");
        assert_eq!(resp.status(), 200);
    })
    .await;

    // One interaction should be stored
    let ctx_id = {
        let ids = store.get_recent_record_ids(1).await.unwrap();
        assert_eq!(ids.len(), 1, "expected one record_id in store");
        ids[0]
    };

    let interactions = store.get_by_record_id(ctx_id).await.unwrap();
    assert_eq!(interactions.len(), 1);

    let i = &interactions[0];
    assert_eq!(i.call_type, CallType::Http);
    assert_eq!(i.sequence, 0);
    assert!(i.fingerprint.contains("GET"));
    assert!(i.fingerprint.contains("{id}"), "fingerprint should normalize the ID segment");
}

#[tokio::test]
async fn replays_http_call_without_hitting_network() {
    let (server, store) = setup().await;

    // Mount a real response — only called during record
    Mock::given(method("POST"))
        .and(path("/v1/charges"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "ch_abc", "status": "succeeded"})),
        )
        .expect(1) // must be called exactly once (during record only)
        .mount(&server)
        .await;

    let client = make_http_client(store.clone());

    // ── record ────────────────────────────────────────────────────────────────
    with_recording(ReplayMode::Record, async {
        client
            .post(format!("{}/v1/charges", server.uri()))
            .json(&json!({"amount": 500}))
            .send()
            .await
            .expect("record request failed");
    })
    .await;

    // Grab the record_id that was just written
    let record_id = store.get_recent_record_ids(1).await.unwrap()[0];

    // ── replay ────────────────────────────────────────────────────────────────
    // Point at a URL that doesn't exist — replay must not make a real call
    use replay_core::context::with_recording_id;
    let resp = with_recording_id(record_id, ReplayMode::Replay, async {
        client
            .post("http://127.0.0.1:1/v1/charges") // port 1 — nothing listening
            .json(&json!({"amount": 500}))
            .send()
            .await
            .expect("replay should return stored response without hitting network")
    })
    .await;

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "ch_abc");
    assert_eq!(body["status"], "succeeded");

    // Verify the mock server was only hit once (record), not during replay
    server.verify().await;
}

#[tokio::test]
async fn passthrough_mode_makes_real_call() {
    let (server, store) = setup().await;

    Mock::given(method("GET"))
        .and(path("/ping"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_http_client(store.clone());

    // Off mode — no MockContext — middleware passes through
    let resp = client
        .get(format!("{}/ping", server.uri()))
        .send()
        .await
        .expect("passthrough request failed");

    assert_eq!(resp.status(), 204);
    server.verify().await;

    // Nothing should be stored
    let ids = store.get_recent_record_ids(10).await.unwrap();
    assert!(ids.is_empty(), "passthrough should not store anything");
}

#[tokio::test]
async fn record_captures_error_responses() {
    let (server, store) = setup().await;

    Mock::given(method("GET"))
        .and(path("/not-found"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(json!({"error": "not_found"})),
        )
        .mount(&server)
        .await;

    let client = make_http_client(store.clone());

    with_recording(ReplayMode::Record, async {
        let resp = client
            .get(format!("{}/not-found", server.uri()))
            .send()
            .await
            .expect("request failed");
        assert_eq!(resp.status(), 404);
    })
    .await;

    let record_id = store.get_recent_record_ids(1).await.unwrap()[0];
    let interactions = store.get_by_record_id(record_id).await.unwrap();
    assert_eq!(interactions[0].response["status"], 404);
}

#[tokio::test]
async fn auth_headers_are_redacted_before_storage() {
    let (server, store) = setup().await;

    Mock::given(method("GET"))
        .and(path("/secure"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&server)
        .await;

    let client = make_http_client(store.clone());

    with_recording(ReplayMode::Record, async {
        client
            .get(format!("{}/secure", server.uri()))
            .header("Authorization", "Bearer test_key_redaction_check")
            .send()
            .await
            .unwrap();
    })
    .await;

    let record_id = store.get_recent_record_ids(1).await.unwrap()[0];
    let interactions = store.get_by_record_id(record_id).await.unwrap();
    let stored_headers = &interactions[0].request["headers"];

    assert!(
        stored_headers.get("authorization").is_none(),
        "authorization header must be redacted before storage"
    );
}
