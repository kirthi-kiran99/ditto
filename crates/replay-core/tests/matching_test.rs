/// Tests for the three-tier matching engine.
///
/// Uses `InMemoryStore` from replay-store — isolated per test via `Arc::new`.

use std::sync::Arc;

use replay_core::{
    CallType, Interaction, InteractionStore, MatchConfig, MatchOutcome, MatchingEngine,
    MissStrategy,
};
use replay_store::InMemoryStore;
use serde_json::json;
use uuid::Uuid;

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_interaction(record_id: Uuid, seq: u32, call_type: CallType, fp: &str) -> Interaction {
    Interaction::new(
        record_id,
        seq,
        call_type,
        fp.to_string(),
        json!({"fp": fp}),
        json!({"ok": true}),
        1,
    )
}

// ── exact match ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn exact_match_returns_exact_outcome() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let fp        = "GET /payments/{id}";

    store.write(&make_interaction(record_id, 3, CallType::Http, fp)).await.unwrap();

    let engine = MatchingEngine::new(store);
    let outcome = engine.resolve(record_id, CallType::Http, fp, 3).await;

    assert!(matches!(outcome, MatchOutcome::Exact(_)));
}

// ── fuzzy match ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn fuzzy_match_within_drift_returns_fuzzy_outcome() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let fp        = "SELECT * FROM payments WHERE id = ?";

    // Recorded at sequence 5; replay asks for sequence 7 (drift = 2)
    store.write(&make_interaction(record_id, 5, CallType::Postgres, fp)).await.unwrap();

    let engine = MatchingEngine::new(store); // default max_sequence_drift = 5
    let outcome = engine.resolve(record_id, CallType::Postgres, fp, 7).await;

    match outcome {
        MatchOutcome::Fuzzy { drift, .. } => assert_eq!(drift, 2),
        other => panic!("expected Fuzzy, got {other:?}"),
    }
}

#[tokio::test]
async fn fuzzy_match_beyond_drift_returns_miss() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let fp        = "GET /charges/{id}";

    // Recorded at sequence 0; replay asks for sequence 20 (drift = 20)
    store.write(&make_interaction(record_id, 0, CallType::Http, fp)).await.unwrap();

    let config = MatchConfig { max_sequence_drift: 5, ..Default::default() };
    let engine = MatchingEngine::with_config(store, config);
    let outcome = engine.resolve(record_id, CallType::Http, fp, 20).await;

    assert!(matches!(outcome, MatchOutcome::Miss));
}

// ── miss ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn empty_store_returns_miss() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();

    let engine = MatchingEngine::new(store);
    let outcome = engine.resolve(record_id, CallType::Http, "GET /payments", 0).await;

    assert!(matches!(outcome, MatchOutcome::Miss));
}

#[tokio::test]
async fn wrong_fingerprint_returns_miss() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();

    store.write(&make_interaction(record_id, 0, CallType::Http, "GET /charges")).await.unwrap();

    let engine = MatchingEngine::new(store);
    // Different fingerprint → no match
    let outcome = engine.resolve(record_id, CallType::Http, "POST /charges", 0).await;

    assert!(matches!(outcome, MatchOutcome::Miss));
}

// ── miss strategy ─────────────────────────────────────────────────────────────

#[test]
fn default_miss_strategies_are_correct() {
    let engine = MatchingEngine::new(Arc::new(InMemoryStore::new()));

    assert_eq!(engine.miss_strategy_for(CallType::Http),     &MissStrategy::Error);
    assert_eq!(engine.miss_strategy_for(CallType::Grpc),     &MissStrategy::Error);
    assert_eq!(engine.miss_strategy_for(CallType::Postgres), &MissStrategy::Error);
    assert_eq!(engine.miss_strategy_for(CallType::Redis),    &MissStrategy::Empty);
    assert_eq!(engine.miss_strategy_for(CallType::Function), &MissStrategy::Passthrough);
}

#[test]
fn custom_miss_strategy_overrides_default() {
    let config = MatchConfig {
        http: MissStrategy::Passthrough,
        ..Default::default()
    };
    let engine = MatchingEngine::with_config(Arc::new(InMemoryStore::new()), config);
    assert_eq!(engine.miss_strategy_for(CallType::Http), &MissStrategy::Passthrough);
    // Other defaults unchanged
    assert_eq!(engine.miss_strategy_for(CallType::Postgres), &MissStrategy::Error);
}

// ── drift boundary ────────────────────────────────────────────────────────────

#[tokio::test]
async fn drift_exactly_at_max_is_accepted() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let fp        = "idempotency:{id}";

    // Recorded at sequence 0; replay asks for sequence 5 (drift == max)
    store.write(&make_interaction(record_id, 0, CallType::Redis, fp)).await.unwrap();

    let config = MatchConfig { max_sequence_drift: 5, ..Default::default() };
    let engine = MatchingEngine::with_config(store, config);
    let outcome = engine.resolve(record_id, CallType::Redis, fp, 5).await;

    assert!(matches!(outcome, MatchOutcome::Fuzzy { drift: 5, .. }));
}

#[tokio::test]
async fn drift_one_over_max_is_miss() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let fp        = "idempotency:{id}";

    store.write(&make_interaction(record_id, 0, CallType::Redis, fp)).await.unwrap();

    let config = MatchConfig { max_sequence_drift: 5, ..Default::default() };
    let engine = MatchingEngine::with_config(store, config);
    let outcome = engine.resolve(record_id, CallType::Redis, fp, 6).await;

    assert!(matches!(outcome, MatchOutcome::Miss));
}

// ── exact beats fuzzy ─────────────────────────────────────────────────────────

#[tokio::test]
async fn exact_match_preferred_over_fuzzy_candidate() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let fp        = "GET /payments/{id}";

    // Two interactions: one at seq 2 (fuzzy candidate) and one at seq 5 (exact)
    store.write(&make_interaction(record_id, 2, CallType::Http, fp)).await.unwrap();
    store.write(&make_interaction(record_id, 5, CallType::Http, fp)).await.unwrap();

    let engine = MatchingEngine::new(store);
    let outcome = engine.resolve(record_id, CallType::Http, fp, 5).await;

    // Should return Exact, not Fuzzy
    assert!(matches!(outcome, MatchOutcome::Exact(_)));
}
