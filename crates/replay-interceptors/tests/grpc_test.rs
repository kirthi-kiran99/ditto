/// gRPC interceptor tests using InMemoryStore.
///
/// These tests do NOT require a real gRPC server. They verify the intercept
/// logic directly: fingerprinting, record path stores interactions, replay
/// path returns stored base64-encoded protobuf bytes.

use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use replay_core::{context::with_recording_store, CallType, Interaction, ReplayMode};
use replay_store::{InMemoryStore, InteractionStore};
use serde_json::json;
use uuid::Uuid;

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_grpc_interaction(
    record_id: Uuid,
    seq:       u32,
    path:      &str,
    body_b64:  &str,
) -> Interaction {
    Interaction::new(
        record_id,
        seq,
        CallType::Grpc,
        path.to_string(),
        json!({"path": path, "body_b64": B64.encode(b"fake-request")}),
        json!({"status": 200, "body_b64": body_b64}),
        5,
    )
}

// ── fingerprint tests ─────────────────────────────────────────────────────────

#[test]
fn grpc_fingerprint_is_stable_path() {
    use replay_core::FingerprintBuilder;

    // gRPC paths are "/package.Service/Method" — no normalization
    assert_eq!(
        FingerprintBuilder::grpc("/payments.PaymentService/CreatePayment"),
        "/payments.PaymentService/CreatePayment"
    );
    assert_eq!(
        FingerprintBuilder::grpc("/grpc.health.v1.Health/Check"),
        "/grpc.health.v1.Health/Check"
    );
}

// ── replay logic tests ────────────────────────────────────────────────────────

#[tokio::test]
async fn replay_returns_stored_body_bytes() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let path      = "/payments.PaymentService/CreatePayment";

    // Simulate a recorded protobuf response (just bytes for testing)
    let fake_proto_bytes: Vec<u8> = vec![0x0a, 0x06, 0x70, 0x61, 0x79, 0x5f, 0x31, 0x32, 0x33];
    let stored_b64 = B64.encode(&fake_proto_bytes);

    let recorded = make_grpc_interaction(record_id, 0, path, &stored_b64);
    store.write(&recorded).await.unwrap();

    with_recording_store(record_id, ReplayMode::Replay, store.clone(), async {
        let slot = replay_core::next_interaction_slot(CallType::Grpc, path.to_string()).unwrap();

        let found = store
            .find_match(slot.record_id, CallType::Grpc, &slot.fingerprint, slot.sequence)
            .await
            .unwrap()
            .unwrap();

        let b64 = found.response["body_b64"].as_str().unwrap();
        let decoded = B64.decode(b64).unwrap();
        assert_eq!(decoded, fake_proto_bytes);
    }).await;
}

#[tokio::test]
async fn replay_falls_back_to_fuzzy_on_sequence_drift() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let path      = "/payments.PaymentService/GetPayment";

    let body_b64 = B64.encode(b"response-bytes");
    // Recorded at sequence 1
    let recorded = make_grpc_interaction(record_id, 1, path, &body_b64);
    store.write(&recorded).await.unwrap();

    // Replay asks for sequence 2 (drift of 1)
    let found = store.find_nearest(record_id, path, 2).await.unwrap();
    assert!(found.is_some(), "fuzzy should bridge drift of 1");
    assert_eq!(
        found.unwrap().response["body_b64"].as_str().unwrap(),
        body_b64
    );
}

#[tokio::test]
async fn multiple_grpc_calls_get_sequential_slots() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();

    with_recording_store(record_id, ReplayMode::Record, store.clone(), async {
        let s1 = replay_core::next_interaction_slot(
            CallType::Grpc, "/payments.PaymentService/CreatePayment".into()
        ).unwrap();
        let s2 = replay_core::next_interaction_slot(
            CallType::Grpc, "/payments.PaymentService/GetPayment".into()
        ).unwrap();
        let s3 = replay_core::next_interaction_slot(
            CallType::Grpc, "/payments.PaymentService/CreatePayment".into()
        ).unwrap();

        assert_eq!(s1.sequence, 0);
        assert_eq!(s2.sequence, 1);
        assert_eq!(s3.sequence, 2);
        assert_eq!(s1.record_id, record_id);
    }).await;
}

#[tokio::test]
async fn grpc_and_http_slots_share_sequence_counter() {
    // Sequence counter is global per recording — all call types share it
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();

    with_recording_store(record_id, ReplayMode::Record, store.clone(), async {
        let s_grpc = replay_core::next_interaction_slot(
            CallType::Grpc, "/svc/Method".into()
        ).unwrap();
        let s_http = replay_core::next_interaction_slot(
            CallType::Http, "GET /health".into()
        ).unwrap();
        let s_grpc2 = replay_core::next_interaction_slot(
            CallType::Grpc, "/svc/Method".into()
        ).unwrap();

        assert_eq!(s_grpc.sequence,  0);
        assert_eq!(s_http.sequence,  1);
        assert_eq!(s_grpc2.sequence, 2);
    }).await;
}

#[tokio::test]
async fn replay_none_when_no_stored_interaction() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();

    with_recording_store(record_id, ReplayMode::Replay, store.clone(), async {
        let slot = replay_core::next_interaction_slot(
            CallType::Grpc, "/payments.PaymentService/CreatePayment".into()
        ).unwrap();

        let found = store
            .find_match(slot.record_id, CallType::Grpc, &slot.fingerprint, slot.sequence)
            .await
            .unwrap();

        assert!(found.is_none(), "empty store should return None");
    }).await;
}
