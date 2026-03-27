/// Redis interceptor tests using InMemoryStore.
///
/// These tests do NOT require a real Redis instance — they verify the intercept
/// logic using the InMemoryStore.
///
/// Integration tests against real Redis are tagged `#[ignore]` and need
/// REPLAY_TEST_REDIS_URL set in the environment.

use std::sync::Arc;

use replay_core::{context::with_recording_store, CallType, Interaction, ReplayMode};
use replay_store::{InMemoryStore, InteractionStore};
use serde_json::json;
use uuid::Uuid;

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_redis_interaction(
    record_id: Uuid,
    seq:       u32,
    fp:        &str,
    response:  serde_json::Value,
) -> Interaction {
    Interaction::new(
        record_id,
        seq,
        CallType::Redis,
        fp.to_string(),
        json!({"op": "GET", "key": fp}),
        response,
        1,
    )
}

// ── fingerprint tests ─────────────────────────────────────────────────────────

#[test]
fn redis_fingerprint_strips_dynamic_segments() {
    use replay_core::FingerprintBuilder;

    assert_eq!(
        FingerprintBuilder::redis_key("payment:session:sess_abc123"),
        "payment:session:{id}"
    );
    assert_eq!(
        FingerprintBuilder::redis_key("idempotency:pay_xyz789"),
        "idempotency:{id}"
    );
    // Short static keys are preserved
    assert_eq!(
        FingerprintBuilder::redis_key("config:global"),
        "config:global"
    );
    assert_eq!(
        FingerprintBuilder::redis_key("feature:flags"),
        "feature:flags"
    );
}

// ── replay logic tests ────────────────────────────────────────────────────────

#[tokio::test]
async fn replay_get_returns_stored_value() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let fp        = "session:{id}";

    let recorded = make_redis_interaction(
        record_id, 0, fp,
        json!({"value": "user_data_xyz"}),
    );
    store.write(&recorded).await.unwrap();

    with_recording_store(record_id, ReplayMode::Replay, store.clone(), async {
        let slot = replay_core::next_interaction_slot(
            CallType::Redis, fp.to_string()
        ).unwrap();

        let found = store
            .find_match(slot.record_id, CallType::Redis, &slot.fingerprint, slot.sequence)
            .await
            .unwrap()
            .unwrap();

        let val = found.response["value"].as_str().unwrap();
        assert_eq!(val, "user_data_xyz");
    }).await;
}

#[tokio::test]
async fn replay_get_returns_none_for_stored_null() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let fp        = "session:{id}";

    // Stored a GET that returned None (key didn't exist)
    let recorded = make_redis_interaction(
        record_id, 0, fp,
        json!({"value": null}),
    );
    store.write(&recorded).await.unwrap();

    with_recording_store(record_id, ReplayMode::Replay, store.clone(), async {
        let slot = replay_core::next_interaction_slot(
            CallType::Redis, fp.to_string()
        ).unwrap();

        let found = store
            .find_match(slot.record_id, CallType::Redis, &slot.fingerprint, slot.sequence)
            .await
            .unwrap()
            .unwrap();

        // Null value → GET returned None originally
        assert!(found.response["value"].is_null());
    }).await;
}

#[tokio::test]
async fn replay_del_returns_stored_result() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let fp        = "idempotency:{id}";

    let recorded = Interaction::new(
        record_id, 0, CallType::Redis, fp.to_string(),
        json!({"op": "DEL", "key": "idempotency:pay_123"}),
        json!({"deleted": true}),
        1,
    );
    store.write(&recorded).await.unwrap();

    with_recording_store(record_id, ReplayMode::Replay, store.clone(), async {
        let slot = replay_core::next_interaction_slot(
            CallType::Redis, fp.to_string()
        ).unwrap();

        let found = store
            .find_match(slot.record_id, CallType::Redis, &slot.fingerprint, slot.sequence)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(found.response["deleted"], true);
    }).await;
}

#[tokio::test]
async fn replay_exists_returns_stored_result() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let fp        = "idempotency:{id}";

    let recorded = Interaction::new(
        record_id, 0, CallType::Redis, fp.to_string(),
        json!({"op": "EXISTS", "key": "idempotency:pay_123"}),
        json!({"exists": true}),
        1,
    );
    store.write(&recorded).await.unwrap();

    with_recording_store(record_id, ReplayMode::Replay, store.clone(), async {
        let slot = replay_core::next_interaction_slot(
            CallType::Redis, fp.to_string()
        ).unwrap();

        let found = store
            .find_match(slot.record_id, CallType::Redis, &slot.fingerprint, slot.sequence)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(found.response["exists"], true);
    }).await;
}

#[tokio::test]
async fn replay_incr_returns_stored_value() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let fp        = "rate:{id}";

    let recorded = Interaction::new(
        record_id, 0, CallType::Redis, fp.to_string(),
        json!({"op": "INCR", "key": "rate:user123"}),
        json!({"value": 5i64}),
        1,
    );
    store.write(&recorded).await.unwrap();

    with_recording_store(record_id, ReplayMode::Replay, store.clone(), async {
        let slot = replay_core::next_interaction_slot(
            CallType::Redis, fp.to_string()
        ).unwrap();

        let found = store
            .find_match(slot.record_id, CallType::Redis, &slot.fingerprint, slot.sequence)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(found.response["value"], 5);
    }).await;
}

#[tokio::test]
async fn mixed_redis_operations_get_sequential_slots() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();

    with_recording_store(record_id, ReplayMode::Record, store.clone(), async {
        let s_get    = replay_core::next_interaction_slot(CallType::Redis, "session:{id}".into()).unwrap();
        let s_exists = replay_core::next_interaction_slot(CallType::Redis, "idempotency:{id}".into()).unwrap();
        let s_set    = replay_core::next_interaction_slot(CallType::Redis, "session:{id}".into()).unwrap();

        assert_eq!(s_get.sequence,    0);
        assert_eq!(s_exists.sequence, 1);
        assert_eq!(s_set.sequence,    2);
    }).await;
}

#[tokio::test]
async fn idempotency_key_replay_flow() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();

    // Simulate the full idempotency check flow:
    //   1. EXISTS check (key not present → false)
    //   2. SET the key
    //   3. Process payment
    //   4. EXISTS check again (key present → true)

    let interactions = vec![
        Interaction::new(record_id, 0, CallType::Redis, "idempotency:{id}".into(),
            json!({"op": "EXISTS", "key": "idempotency:pay_abc"}),
            json!({"exists": false}), 1),
        Interaction::new(record_id, 1, CallType::Redis, "idempotency:{id}".into(),
            json!({"op": "SET",    "key": "idempotency:pay_abc", "value": "processing", "ttl": 300}),
            json!({"ok": true}), 1),
        Interaction::new(record_id, 2, CallType::Http, "POST /v1/charges".into(),
            json!({"method": "POST", "url": "/v1/charges"}),
            json!({"status": 200, "body": {"id": "ch_123"}}), 50),
        Interaction::new(record_id, 3, CallType::Redis, "idempotency:{id}".into(),
            json!({"op": "SET",    "key": "idempotency:pay_abc", "value": "done", "ttl": 86400}),
            json!({"ok": true}), 1),
    ];

    for i in &interactions {
        store.write(i).await.unwrap();
    }

    // Verify replay finds all 4 in correct order
    let all = store.get_by_record_id(record_id).await.unwrap();
    assert_eq!(all.len(), 4);
    assert_eq!(all[0].response["exists"], false);
    assert_eq!(all[1].request["op"], "SET");
    assert_eq!(all[2].call_type, CallType::Http);
    assert_eq!(all[3].request["value"], "done");
}
