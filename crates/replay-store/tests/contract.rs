/// Store contract tests — the same test suite runs against every
/// `InteractionStore` implementation.
///
/// # Adding a new backend
/// Use the `store_contract_tests!` macro:
///
/// ```ignore
/// store_contract_tests!(my_backend, MyStore::new().await);
/// ```
///
/// # Postgres backend
/// Set `TEST_DB_URL` to a Postgres connection string and run with the
/// `postgres-tests` feature to exercise `PostgresStore`.
///
/// ```text
/// TEST_DB_URL=postgres://... cargo test -p replay-store --features postgres-tests
/// ```

use replay_core::{CallType, Interaction, InteractionStore};
use replay_store::InMemoryStore;
use serde_json::json;
use uuid::Uuid;

// ── helpers ───────────────────────────────────────────────────────────────────

fn make(record_id: Uuid, seq: u32, call_type: CallType, fp: &str) -> Interaction {
    Interaction::new(
        record_id,
        seq,
        call_type,
        fp.to_string(),
        json!({}),
        json!({ "seq": seq }),
        10,
    )
}

// ── contract macro ────────────────────────────────────────────────────────────

/// Run the full store contract test suite against the given store expression.
///
/// Each invocation creates an isolated `mod` so test names never clash even
/// when multiple backends are tested in the same binary.
macro_rules! store_contract_tests {
    ($mod_name:ident, $store_expr:expr) => {
        mod $mod_name {
            use super::*;

            async fn fresh() -> impl InteractionStore {
                $store_expr
            }

            // ── write + read ──────────────────────────────────────────────────

            #[tokio::test]
            async fn write_and_read_by_record_id() {
                let store = fresh().await;
                let id    = Uuid::new_v4();
                store.write(&make(id, 0, CallType::Http, "GET /a")).await.unwrap();
                store.write(&make(id, 1, CallType::Http, "GET /a")).await.unwrap();

                let found = store.get_by_record_id(id).await.unwrap();
                assert_eq!(found.len(), 2);
                assert_eq!(found[0].sequence, 0);
                assert_eq!(found[1].sequence, 1);
            }

            #[tokio::test]
            async fn write_batch_stores_all_interactions() {
                let store = fresh().await;
                let id    = Uuid::new_v4();
                let batch: Vec<_> = (0u32..5)
                    .map(|s| make(id, s, CallType::Http, "GET /b"))
                    .collect();
                store.write_batch(&batch).await.unwrap();

                let found = store.get_by_record_id(id).await.unwrap();
                assert_eq!(found.len(), 5);
            }

            #[tokio::test]
            async fn unknown_record_id_returns_empty_vec() {
                let store = fresh().await;
                let found = store.get_by_record_id(Uuid::new_v4()).await.unwrap();
                assert!(found.is_empty());
            }

            // ── get_entry_point ───────────────────────────────────────────────

            #[tokio::test]
            async fn get_entry_point_returns_sequence_zero() {
                let store = fresh().await;
                let id    = Uuid::new_v4();
                for seq in [3u32, 1, 0, 2] {
                    store.write(&make(id, seq, CallType::Http, "GET /ep")).await.unwrap();
                }

                let ep = store.get_entry_point(id).await.unwrap();
                assert!(ep.is_some());
                assert_eq!(ep.unwrap().sequence, 0);
            }

            #[tokio::test]
            async fn get_entry_point_returns_none_for_unknown_id() {
                let store = fresh().await;
                let ep = store.get_entry_point(Uuid::new_v4()).await.unwrap();
                assert!(ep.is_none());
            }

            // ── find_match ────────────────────────────────────────────────────

            #[tokio::test]
            async fn find_match_exact_hit() {
                let store = fresh().await;
                let id    = Uuid::new_v4();
                store.write(&make(id, 0, CallType::Http,     "GET /pay")).await.unwrap();
                store.write(&make(id, 1, CallType::Postgres, "SELECT ?")).await.unwrap();

                let m = store.find_match(id, CallType::Http, "GET /pay", 0).await.unwrap();
                assert!(m.is_some());
                assert_eq!(m.unwrap().sequence, 0);
            }

            #[tokio::test]
            async fn find_match_wrong_sequence_returns_none() {
                let store = fresh().await;
                let id    = Uuid::new_v4();
                store.write(&make(id, 0, CallType::Http, "GET /pay")).await.unwrap();

                let m = store.find_match(id, CallType::Http, "GET /pay", 5).await.unwrap();
                assert!(m.is_none(), "wrong sequence must not match");
            }

            #[tokio::test]
            async fn find_match_wrong_call_type_returns_none() {
                let store = fresh().await;
                let id    = Uuid::new_v4();
                store.write(&make(id, 0, CallType::Http, "GET /pay")).await.unwrap();

                let m = store.find_match(id, CallType::Postgres, "GET /pay", 0).await.unwrap();
                assert!(m.is_none(), "wrong call_type must not match");
            }

            // ── find_nearest ──────────────────────────────────────────────────

            #[tokio::test]
            async fn find_nearest_bridges_sequence_drift() {
                let store = fresh().await;
                let id    = Uuid::new_v4();
                // Recorded at sequence 2; new build calls at sequence 3.
                store.write(&make(id, 2, CallType::Http, "POST /charges")).await.unwrap();

                let m = store.find_nearest(id, "POST /charges", 3).await.unwrap();
                assert!(m.is_some());
                assert_eq!(m.unwrap().sequence, 2, "nearest should bridge drift of 1");
            }

            #[tokio::test]
            async fn find_nearest_no_fingerprint_match_returns_none() {
                let store = fresh().await;
                let id    = Uuid::new_v4();
                store.write(&make(id, 0, CallType::Http, "GET /foo")).await.unwrap();

                let m = store.find_nearest(id, "GET /bar", 0).await.unwrap();
                assert!(m.is_none());
            }

            // ── get_recent_record_ids ─────────────────────────────────────────

            #[tokio::test]
            async fn get_recent_record_ids_returns_up_to_limit() {
                let store = fresh().await;
                for _ in 0..7 {
                    let id = Uuid::new_v4();
                    store.write(&make(id, 0, CallType::Http, "GET /")).await.unwrap();
                }

                let ids = store.get_recent_record_ids(5).await.unwrap();
                assert!(ids.len() <= 5, "must respect the limit");
            }

            #[tokio::test]
            async fn get_recent_record_ids_empty_when_nothing_stored() {
                let store = fresh().await;
                let ids = store.get_recent_record_ids(10).await.unwrap();
                assert!(ids.is_empty());
            }

            // ── isolation: different record_ids don't interfere ───────────────

            #[tokio::test]
            async fn different_record_ids_are_independent() {
                let store = fresh().await;
                let id_a  = Uuid::new_v4();
                let id_b  = Uuid::new_v4();

                store.write(&make(id_a, 0, CallType::Http, "GET /a")).await.unwrap();
                store.write(&make(id_b, 0, CallType::Redis, "GET key")).await.unwrap();

                let a = store.get_by_record_id(id_a).await.unwrap();
                let b = store.get_by_record_id(id_b).await.unwrap();

                assert_eq!(a.len(), 1);
                assert_eq!(b.len(), 1);
                assert_eq!(a[0].call_type, CallType::Http);
                assert_eq!(b[0].call_type, CallType::Redis);
            }
        }
    };
}

// ── InMemoryStore — always runs ───────────────────────────────────────────────

store_contract_tests!(in_memory, InMemoryStore::new());

// ── Additional InMemoryStore-specific tests ───────────────────────────────────

#[test]
fn in_memory_seed_populates_store() {
    let store = InMemoryStore::new();
    let id    = Uuid::new_v4();
    let interactions: Vec<_> = (0u32..3)
        .map(|s| make(id, s, CallType::Http, "GET /seed"))
        .collect();

    store.seed(interactions);
    assert_eq!(store.len(), 3);
    assert!(!store.is_empty());
}

#[test]
fn in_memory_all_returns_every_interaction() {
    let store = InMemoryStore::new();
    let id_a  = Uuid::new_v4();
    let id_b  = Uuid::new_v4();

    store.seed(vec![
        make(id_a, 0, CallType::Http,  "GET /a"),
        make(id_a, 1, CallType::Redis, "GET k"),
        make(id_b, 0, CallType::Http,  "POST /b"),
    ]);

    assert_eq!(store.len(), 3);
    let all = store.all();
    assert_eq!(all.len(), 3);
}

#[test]
fn in_memory_is_empty_on_new() {
    assert!(InMemoryStore::new().is_empty());
}

#[tokio::test]
async fn in_memory_seed_then_replay_returns_seeded_responses() {
    use replay_core::MOCK_CTX;

    let store     = std::sync::Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();

    let stored_body = json!({ "status": 200, "body": "stored" });
    store.seed(vec![Interaction::new(
        record_id,
        0,
        CallType::Http,
        "GET /test".to_string(),
        json!({}),
        stored_body.clone(),
        5,
    )]);

    // find_match should return the seeded interaction
    let found = store
        .find_match(record_id, CallType::Http, "GET /test", 0)
        .await
        .unwrap();

    assert!(found.is_some());
    assert_eq!(found.unwrap().response, stored_body);
}

// ── PostgresStore — only when TEST_DB_URL is set ──────────────────────────────
//
// To run:  TEST_DB_URL=postgres://... cargo test -p replay-store
//
// These tests are commented out to keep CI green without a database.
// Uncomment and add the `postgres-tests` feature gate when a test DB is available.
//
// #[cfg(feature = "postgres-tests")]
// store_contract_tests!(postgres, {
//     let url = std::env::var("TEST_DB_URL").expect("TEST_DB_URL not set");
//     replay_store::PostgresStore::new(&url).await.expect("failed to connect")
// });
