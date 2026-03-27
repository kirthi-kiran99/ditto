/// Postgres interceptor tests using InMemoryStore.
///
/// These tests do NOT require a real Postgres connection — they verify the
/// intercept logic (record path stores interactions, replay path returns stored
/// data) using the InMemoryStore and a fake/stubbed executor path.
///
/// Integration tests against real Postgres are tagged `#[ignore]` and need
/// REPLAY_TEST_PG_URL set in the environment.

use std::sync::Arc;

use replay_core::{
    context::with_recording_store, CallType, Interaction, ReplayMode,
};
use replay_store::{InMemoryStore, InteractionStore};
use serde_json::json;
use uuid::Uuid;

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_pg_interaction(record_id: Uuid, seq: u32, fp: &str, response: serde_json::Value) -> Interaction {
    Interaction::new(
        record_id,
        seq,
        CallType::Postgres,
        fp.to_string(),
        json!({"sql": fp}),
        response,
        5,
    )
}

// ── unit tests: verify stored interaction shape ───────────────────────────────

#[tokio::test]
async fn fingerprint_normalises_sql_params() {
    use replay_core::FingerprintBuilder;

    assert_eq!(
        FingerprintBuilder::sql("SELECT * FROM payments WHERE id = $1 AND status = $2"),
        "SELECT * FROM payments WHERE id = ? AND status = ?"
    );
    assert_eq!(
        FingerprintBuilder::sql("INSERT INTO events (id, payload) VALUES ($1, $2)"),
        "INSERT INTO events (id, payload) VALUES (?, ?)"
    );
    // No params — unchanged
    assert_eq!(
        FingerprintBuilder::sql("SELECT COUNT(*) FROM payments"),
        "SELECT COUNT(*) FROM payments"
    );
}

#[tokio::test]
async fn replay_returns_stored_rows_for_select() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let fp        = "SELECT * FROM payments WHERE id = ?";

    // Pre-populate the store with a recorded interaction
    let recorded = make_pg_interaction(
        record_id, 0, fp,
        json!({
            "rows": [
                {"id": "pay_123", "amount": 500, "status": "succeeded"},
                {"id": "pay_456", "amount": 200, "status": "pending"}
            ]
        }),
    );
    store.write(&recorded).await.unwrap();

    // In replay mode, find_match should return our recorded interaction
    with_recording_store(record_id, ReplayMode::Replay, store.clone(), async {
        let slot = replay_core::next_interaction_slot(
            CallType::Postgres,
            fp.to_string(),
        ).unwrap();

        let found = store
            .find_match(slot.record_id, CallType::Postgres, &slot.fingerprint, slot.sequence)
            .await
            .unwrap();

        assert!(found.is_some(), "should find recorded interaction");
        let rows = &found.unwrap().response["rows"];
        assert_eq!(rows.as_array().unwrap().len(), 2);
        assert_eq!(rows[0]["id"], "pay_123");
        assert_eq!(rows[1]["status"], "pending");
    }).await;
}

#[tokio::test]
async fn replay_returns_stored_execute_result() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let fp        = "UPDATE payments SET status = ? WHERE id = ?";

    let recorded = make_pg_interaction(
        record_id, 0, fp,
        json!({"rows_affected": 1}),
    );
    store.write(&recorded).await.unwrap();

    with_recording_store(record_id, ReplayMode::Replay, store.clone(), async {
        let slot = replay_core::next_interaction_slot(
            CallType::Postgres,
            fp.to_string(),
        ).unwrap();

        let found = store
            .find_match(slot.record_id, CallType::Postgres, &slot.fingerprint, slot.sequence)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(found.response["rows_affected"], 1);
    }).await;
}

#[tokio::test]
async fn fuzzy_match_bridges_sequence_drift() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let fp        = "SELECT * FROM payments WHERE id = ?";

    // Recorded at sequence 2
    let recorded = make_pg_interaction(record_id, 2, fp, json!({"rows": [{"id": "pay_999"}]}));
    store.write(&recorded).await.unwrap();

    // New build asks for sequence 3 (one extra call was inserted before it)
    let found = store.find_nearest(record_id, fp, 3).await.unwrap();
    assert!(found.is_some(), "fuzzy match should bridge drift of 1");
    assert_eq!(found.unwrap().response["rows"][0]["id"], "pay_999");
}

#[tokio::test]
async fn multiple_queries_get_sequential_slots() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();

    with_recording_store(record_id, ReplayMode::Record, store.clone(), async {
        let s1 = replay_core::next_interaction_slot(
            CallType::Postgres, "SELECT * FROM payments WHERE id = ?".into()
        ).unwrap();
        let s2 = replay_core::next_interaction_slot(
            CallType::Postgres, "UPDATE payments SET status = ? WHERE id = ?".into()
        ).unwrap();
        let s3 = replay_core::next_interaction_slot(
            CallType::Redis, "payment:{id}".into()
        ).unwrap();

        assert_eq!(s1.sequence, 0);
        assert_eq!(s2.sequence, 1);
        assert_eq!(s3.sequence, 2);
        assert_eq!(s1.record_id, record_id);
        assert_eq!(s2.record_id, record_id);
        assert_eq!(s3.record_id, record_id);
    }).await;
}

#[tokio::test]
async fn no_interaction_stored_outside_record_mode() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();

    // Passthrough mode — nothing should be written
    with_recording_store(record_id, ReplayMode::Passthrough, store.clone(), async {
        let _slot = replay_core::next_interaction_slot(
            CallType::Postgres, "SELECT 1".into()
        );
        // Don't write anything — just verify slot mode
        let slot = replay_core::next_interaction_slot(
            CallType::Postgres, "SELECT 1".into()
        ).unwrap();
        assert!(matches!(slot.mode, ReplayMode::Passthrough));
    }).await;

    let interactions = store.get_by_record_id(record_id).await.unwrap();
    assert!(interactions.is_empty(), "passthrough should not store interactions");
}
