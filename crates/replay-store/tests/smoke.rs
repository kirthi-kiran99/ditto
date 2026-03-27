use replay_core::{CallType, Interaction};
use replay_store::{InMemoryStore, InteractionStore};
use serde_json::json;
use uuid::Uuid;

fn make(record_id: Uuid, seq: u32, fp: &str) -> Interaction {
    Interaction::new(record_id, seq, CallType::Http, fp.to_string(), json!({}), json!({}), 10)
}

#[tokio::test]
async fn write_1000_and_read_back() {
    let store = InMemoryStore::new();
    let id = Uuid::new_v4();

    for seq in 0..1000u32 {
        store.write(&make(id, seq, "GET /api/{id}")).await.unwrap();
    }

    let results = store.get_by_record_id(id).await.unwrap();
    assert_eq!(results.len(), 1000);
    assert_eq!(results.first().unwrap().sequence, 0);
    assert_eq!(results.last().unwrap().sequence, 999);
}

#[tokio::test]
async fn find_nearest_drift_of_two() {
    let store = InMemoryStore::new();
    let id = Uuid::new_v4();

    // record at sequences 0, 2, 4
    for seq in [0u32, 2, 4] {
        store.write(&make(id, seq, "POST /charges")).await.unwrap();
    }

    // ask for sequence 3 — nearest is 2 (drift=1) or 4 (drift=1)
    let m = store.find_nearest(id, "POST /charges", 3).await.unwrap();
    assert!(m.is_some());
    let seq = m.unwrap().sequence;
    assert!(seq == 2 || seq == 4, "expected 2 or 4, got {seq}");
}

#[tokio::test]
async fn get_entry_point_returns_seq_zero() {
    let store = InMemoryStore::new();
    let id = Uuid::new_v4();
    for seq in 0..5u32 {
        store.write(&make(id, seq, "GET /foo")).await.unwrap();
    }
    let entry = store.get_entry_point(id).await.unwrap();
    assert_eq!(entry.unwrap().sequence, 0);
}
