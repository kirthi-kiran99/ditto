use replay_core::{
    Interaction, MockContext, ReplayMode, CallType, CallStatus,
    FingerprintBuilder, with_recording, current_ctx,
};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn interaction_round_trips_serde() {
    let id = Uuid::new_v4();
    let record_id = Uuid::new_v4();
    let i = Interaction::new(
        record_id,
        0,
        CallType::Http,
        "GET /api/v1/payments/{id}".to_string(),
        json!({"method": "GET", "url": "/api/v1/payments/pay_123"}),
        json!({"status": 200, "body": {"id": "pay_123"}}),
        42,
    );
    let json = serde_json::to_string(&i).unwrap();
    let i2: Interaction = serde_json::from_str(&json).unwrap();
    assert_eq!(i.id, i2.id);
    assert_eq!(i.record_id, i2.record_id);
    assert_eq!(i.fingerprint, i2.fingerprint);
    assert_eq!(i.call_type, i2.call_type);
    assert_eq!(i.sequence, i2.sequence);
}

#[tokio::test]
async fn mock_context_sequence_counter() {
    let ctx = MockContext::new(ReplayMode::Record);
    assert_eq!(ctx.next_seq(), 0);
    assert_eq!(ctx.next_seq(), 1);
    assert_eq!(ctx.next_seq(), 2);
}

#[tokio::test]
async fn with_recording_sets_context() {
    let result = with_recording(ReplayMode::Record, async {
        let ctx = current_ctx().unwrap();
        assert!(matches!(ctx.mode, ReplayMode::Record));
        99u32
    })
    .await;
    assert_eq!(result, 99);
}

#[test]
fn fingerprint_strips_sql_params() {
    assert_eq!(
        FingerprintBuilder::sql("SELECT * FROM payments WHERE id = $1"),
        "SELECT * FROM payments WHERE id = ?"
    );
}

#[test]
fn fingerprint_http_strips_id_segment() {
    assert_eq!(
        FingerprintBuilder::http("GET", "/api/v1/payments/pay_abc123"),
        "GET /api/v1/payments/{id}"
    );
}
