use std::sync::Arc;

use replay_core::{with_recording, with_recording_store, CallType, ReplayMode};
use replay_macro::record_io;
use replay_store::{InMemoryStore, InteractionStore};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── annotated functions under test ───────────────────────────────────────────

#[record_io]
async fn compute_fee(amount: u64, currency: String) -> u64 {
    amount * 2 + 100
}

#[record_io]
async fn echo_string(s: String) -> String {
    format!("echo: {s}")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct PaymentResult {
    id:     String,
    status: String,
    fee:    u64,
}

#[record_io]
async fn process_payment(amount: u64) -> PaymentResult {
    PaymentResult {
        id:     format!("pay_{amount}"),
        status: "succeeded".to_string(),
        fee:    amount / 10,
    }
}

#[record_io]
async fn returns_unit(x: u32) {
    let _ = x;
}

// ── helper ────────────────────────────────────────────────────────────────────

fn fresh_store() -> Arc<InMemoryStore> {
    Arc::new(InMemoryStore::new())
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn record_captures_return_value() {
    let store = fresh_store();
    let id    = Uuid::new_v4();

    with_recording_store(id, ReplayMode::Record, store.clone(), async {
        let fee = compute_fee(500, "USD".to_string()).await;
        assert_eq!(fee, 1100); // 500*2 + 100
    })
    .await;

    let interactions = store.get_by_record_id(id).await.unwrap();
    assert_eq!(interactions.len(), 1);

    let i = &interactions[0];
    assert_eq!(i.call_type, CallType::Function);
    assert!(i.fingerprint.ends_with("::compute_fee"), "fingerprint: {}", i.fingerprint);
    assert_eq!(i.request["amount"], 500);
    assert_eq!(i.request["currency"], "USD");
    assert_eq!(i.response, serde_json::json!(1100u64));
}

#[tokio::test]
async fn replay_returns_stored_value_without_running_body() {
    let store = fresh_store();
    let id    = Uuid::new_v4();

    // Record
    with_recording_store(id, ReplayMode::Record, store.clone(), async {
        let fee = compute_fee(500, "USD".to_string()).await;
        assert_eq!(fee, 1100);
    })
    .await;

    assert_eq!(store.get_by_record_id(id).await.unwrap().len(), 1);

    // Replay — body must NOT execute; stored value returned
    let replayed = with_recording_store(id, ReplayMode::Replay, store.clone(), async {
        compute_fee(500, "USD".to_string()).await
    })
    .await;

    assert_eq!(replayed, 1100);
}

#[tokio::test]
async fn replay_struct_return_type() {
    let store = fresh_store();
    let id    = Uuid::new_v4();

    with_recording_store(id, ReplayMode::Record, store.clone(), async {
        let r = process_payment(1000).await;
        assert_eq!(r.fee, 100);
    })
    .await;

    let replayed = with_recording_store(id, ReplayMode::Replay, store.clone(), async {
        process_payment(1000).await
    })
    .await;

    assert_eq!(replayed.id, "pay_1000");
    assert_eq!(replayed.status, "succeeded");
    assert_eq!(replayed.fee, 100);
}

#[tokio::test]
async fn replay_string_return_type() {
    let store = fresh_store();
    let id    = Uuid::new_v4();

    with_recording_store(id, ReplayMode::Record, store.clone(), async {
        let s = echo_string("hello".to_string()).await;
        assert_eq!(s, "echo: hello");
    })
    .await;

    let replayed = with_recording_store(id, ReplayMode::Replay, store.clone(), async {
        echo_string("hello".to_string()).await
    })
    .await;

    assert_eq!(replayed, "echo: hello");
}

#[tokio::test]
async fn passthrough_mode_executes_real_body() {
    let fee = with_recording(ReplayMode::Passthrough, async {
        compute_fee(200, "EUR".to_string()).await
    })
    .await;
    assert_eq!(fee, 500); // 200*2 + 100
}

#[tokio::test]
async fn no_context_executes_real_body() {
    let fee = compute_fee(100, "GBP".to_string()).await;
    assert_eq!(fee, 300); // 100*2 + 100
}

#[tokio::test]
async fn unit_return_type_records_and_replays() {
    let store = fresh_store();
    let id    = Uuid::new_v4();

    with_recording_store(id, ReplayMode::Record, store.clone(), async {
        returns_unit(42).await;
    })
    .await;

    let interactions = store.get_by_record_id(id).await.unwrap();
    assert_eq!(interactions.len(), 1, "unit fn should still be recorded");

    // Replay must complete without panic
    with_recording_store(id, ReplayMode::Replay, store.clone(), async {
        returns_unit(42).await;
    })
    .await;
}

#[tokio::test]
async fn multiple_annotated_calls_get_sequential_slots() {
    let store = fresh_store();
    let id    = Uuid::new_v4();

    with_recording_store(id, ReplayMode::Record, store.clone(), async {
        let _ = compute_fee(100, "USD".to_string()).await;
        let _ = echo_string("a".to_string()).await;
        let _ = compute_fee(200, "EUR".to_string()).await;
    })
    .await;

    let interactions = store.get_by_record_id(id).await.unwrap();
    assert_eq!(interactions.len(), 3);
    assert_eq!(interactions[0].sequence, 0);
    assert_eq!(interactions[1].sequence, 1);
    assert_eq!(interactions[2].sequence, 2);
}

#[tokio::test]
async fn replay_all_three_calls_in_sequence() {
    let store = fresh_store();
    let id    = Uuid::new_v4();

    with_recording_store(id, ReplayMode::Record, store.clone(), async {
        let _ = compute_fee(100, "USD".to_string()).await;
        let _ = echo_string("hello".to_string()).await;
        let _ = compute_fee(200, "EUR".to_string()).await;
    })
    .await;

    let (fee1, echo, fee2) = with_recording_store(id, ReplayMode::Replay, store.clone(), async {
        let a = compute_fee(100, "USD".to_string()).await;
        let b = echo_string("hello".to_string()).await;
        let c = compute_fee(200, "EUR".to_string()).await;
        (a, b, c)
    })
    .await;

    assert_eq!(fee1, 300);         // 100*2+100
    assert_eq!(echo, "echo: hello");
    assert_eq!(fee2, 500);         // 200*2+100
}
