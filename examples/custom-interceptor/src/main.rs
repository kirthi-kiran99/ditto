//! Custom interceptor example — Elasticsearch-style client.
//!
//! Demonstrates how to implement [`ReplayInterceptor`] for a client library
//! that is not covered by the built-in interceptors in `replay-interceptors`.
//!
//! The "Elasticsearch" client here is a stub — it records/replays the shape
//! of what a real implementation would do without requiring an actual cluster.
//!
//! # Key concepts
//!
//! 1. **Request/response types** — define types that can be serialized to JSON
//!    so the store can persist them.
//! 2. **Fingerprinting** — strip volatile per-request values (IDs, dates) so
//!    the same logical call produces the same fingerprint across requests.
//! 3. **Normalization** — strip timing/shard fields from responses so replayed
//!    values match recorded ones even if the cluster state changed.
//! 4. **`InterceptorRunner`** — wraps the interceptor, handles all store I/O
//!    and mode switching.
//!
//! # Running
//! ```bash
//! # Record mode (stores calls in InMemoryStore, prints them after)
//! cargo run --example custom-interceptor
//!
//! # Replay mode (serves from seeded store, no real I/O)
//! REPLAY_MODE=replay cargo run --example custom-interceptor
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use replay_core::{
    CallType, FingerprintBuilder, InteractionStore, InterceptorRunner, MockContext,
    ReplayError, ReplayInterceptor, ReplayMode, MOCK_CTX,
};
use replay_store::InMemoryStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

// ── request / response types ──────────────────────────────────────────────────

/// Normalised Elasticsearch search request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EsRequest {
    pub index:  String,
    pub method: String,
    pub body:   Value,
}

/// Elasticsearch search response — volatile fields (`took`, `_shards`) are
/// excluded to prevent spurious diff failures in replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EsResponse {
    pub hits:  Value,
    pub total: u64,
}

// ── error type ────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum EsError {
    /// Bridge ReplayError so InterceptorRunner can surface store misses.
    #[error(transparent)]
    Replay(#[from] ReplayError),

    /// Wrap real client errors.
    #[error("elasticsearch error: {0}")]
    Client(String),
}

// ── stub Elasticsearch client ─────────────────────────────────────────────────

/// Stand-in for `elasticsearch::Elasticsearch`. In a real codebase, replace
/// this with the actual client.
pub struct FakeEsClient;

impl FakeEsClient {
    /// Simulates a search — always returns the same two hits.
    async fn search(&self, req: &EsRequest) -> Result<EsResponse, EsError> {
        // Pretend we made a network call.
        println!("  [real I/O] searching index={} method={}", req.index, req.method);
        Ok(EsResponse {
            total: 2,
            hits:  json!([
                { "id": "pay_001", "amount": 1000, "currency": "USD" },
                { "id": "pay_002", "amount": 2500, "currency": "EUR" },
            ]),
        })
    }
}

// ── interceptor ───────────────────────────────────────────────────────────────

pub struct EsInterceptor {
    client: FakeEsClient,
}

impl EsInterceptor {
    pub fn new() -> Self {
        Self { client: FakeEsClient }
    }
}

#[async_trait]
impl ReplayInterceptor for EsInterceptor {
    type Request  = EsRequest;
    type Response = EsResponse;
    type Error    = EsError;

    fn call_type(&self) -> CallType {
        CallType::Http
    }

    /// Produces a stable fingerprint by:
    /// - Normalising the index name (strip date/numeric suffixes)
    /// - Keeping the method and path structure
    fn fingerprint(&self, req: &EsRequest) -> String {
        let index = normalize_index(&req.index);
        FingerprintBuilder::http(&req.method, &format!("/{index}/_search"))
    }

    /// Strip volatile request fields (scroll_id, preference, routing).
    fn normalize_request(&self, req: &EsRequest) -> Value {
        let mut body = req.body.clone();
        strip_keys(&mut body, &["scroll_id", "preference", "routing"]);
        json!({
            "method": req.method,
            "index":  normalize_index(&req.index),
            "body":   body,
        })
    }

    /// `EsResponse` already excludes `took` and `_shards` — just serialize.
    fn normalize_response(&self, res: &EsResponse) -> Value {
        serde_json::to_value(res).unwrap_or(Value::Null)
    }

    async fn execute(&self, req: Self::Request) -> Result<EsResponse, EsError> {
        self.client.search(&req).await
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Strip date/numeric suffixes from Elasticsearch index names so recordings
/// stay valid across index rollovers.
///
/// ```
/// assert_eq!(normalize_index("payments-2024.01.15"), "payments-{id}");
/// assert_eq!(normalize_index("logs-prod-42"),         "logs-prod-{id}");
/// assert_eq!(normalize_index("stable-index"),         "stable-index");
/// ```
fn normalize_index(index: &str) -> String {
    let parts: Vec<&str> = index.split('-').collect();
    // Only strip the trailing segment if there is a meaningful prefix.
    if parts.len() >= 2 {
        if let Some(last) = parts.last() {
            let looks_volatile = last.len() > 3
                && last.chars().all(|c| c.is_ascii_alphanumeric() || c == '.');
            if looks_volatile {
                let prefix = parts[..parts.len() - 1].join("-");
                return format!("{prefix}-{{id}}");
            }
        }
    }
    index.to_string()
}

/// Remove listed keys from a JSON object in-place.
fn strip_keys(value: &mut Value, keys: &[&str]) {
    if let Some(obj) = value.as_object_mut() {
        for key in keys {
            obj.remove(*key);
        }
    }
}

// ── application wiring ────────────────────────────────────────────────────────

fn build_runner(store: Arc<dyn InteractionStore>) -> InterceptorRunner<EsInterceptor> {
    InterceptorRunner::new(EsInterceptor::new(), store)
}

// ── main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let store     = Arc::new(InMemoryStore::new());
    let record_id = Uuid::new_v4();
    let mode = match std::env::var("REPLAY_MODE").as_deref() {
        Ok("record")  => ReplayMode::Record,
        Ok("replay")  => ReplayMode::Replay,
        _             => ReplayMode::Off,
    };

    println!("mode: {mode:?}  record_id: {record_id}");

    // Seed the store so Replay mode has something to return.
    if matches!(mode, ReplayMode::Replay) {
        seed_store(&store, record_id);
    }

    let runner = build_runner(store.clone() as Arc<dyn InteractionStore>);

    let ctx = MockContext::with_id(record_id, mode.clone());
    let response = MOCK_CTX
        .scope(ctx, async {
            runner
                .run(EsRequest {
                    index:  "payments-2024.01".to_string(),
                    method: "POST".to_string(),
                    body:   json!({ "query": { "match_all": {} }, "size": 10 }),
                })
                .await
        })
        .await;

    match response {
        Ok(resp) => {
            println!("response: total={} hits={}", resp.total, resp.hits);
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }

    // After Record mode, show what was stored.
    if !matches!(mode, ReplayMode::Replay) {
        let all = store.all();
        println!("\nstored {} interaction(s):", all.len());
        for i in &all {
            println!("  seq={} fp={}", i.sequence, i.fingerprint);
        }
    }
}

fn seed_store(store: &InMemoryStore, record_id: Uuid) {
    use replay_core::{CallStatus, Interaction};
    store.seed(vec![Interaction {
        id:           Uuid::new_v4(),
        record_id,
        parent_id:    None,
        sequence:     0,
        call_type:    CallType::Http,
        fingerprint:  "POST /payments-{id}/_search".to_string(),
        request:      json!({ "method": "POST", "index": "payments-{id}" }),
        response:     json!({
            "total": 2,
            "hits": [
                { "id": "pay_001", "amount": 1000, "currency": "USD" },
                { "id": "pay_002", "amount": 2500, "currency": "EUR" },
            ]
        }),
        duration_ms:  8,
        status:       CallStatus::Completed,
        error:        None,
        recorded_at:  chrono::Utc::now(),
        build_hash:   String::new(),
        service_name: String::new(),
    }]);
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use replay_core::{InteractionStore, MockContext, ReplayMode, MOCK_CTX};
    use replay_store::InMemoryStore;
    use std::sync::Arc;

    #[test]
    fn normalize_index_strips_date_suffix() {
        assert_eq!(normalize_index("payments-2024.01.15"), "payments-{id}");
        assert_eq!(normalize_index("logs-4200"),           "logs-{id}");
        assert_eq!(normalize_index("stable"),              "stable");
    }

    #[tokio::test]
    async fn record_mode_writes_interaction_to_store() {
        let store     = Arc::new(InMemoryStore::new());
        let record_id = Uuid::new_v4();
        let runner    = build_runner(store.clone() as Arc<dyn InteractionStore>);
        let ctx       = MockContext::with_id(record_id, ReplayMode::Record);

        MOCK_CTX
            .scope(ctx, async {
                let resp = runner
                    .run(EsRequest {
                        index:  "payments-2024.01".to_string(),
                        method: "POST".to_string(),
                        body:   json!({ "query": { "match_all": {} } }),
                    })
                    .await
                    .unwrap();
                assert_eq!(resp.total, 2);
            })
            .await;

        let interactions = store.get_by_record_id(record_id).await.unwrap();
        assert_eq!(interactions.len(), 1);
        assert_eq!(interactions[0].call_type, CallType::Http);
        assert!(interactions[0].fingerprint.contains("payments"));
    }

    #[tokio::test]
    async fn replay_mode_returns_stored_response_without_real_call() {
        let store     = Arc::new(InMemoryStore::new());
        let record_id = Uuid::new_v4();

        // Seed the store with a pre-recorded response.
        seed_store(&store, record_id);

        let runner = build_runner(store.clone() as Arc<dyn InteractionStore>);
        let ctx    = MockContext::with_id(record_id, ReplayMode::Replay);

        let resp = MOCK_CTX
            .scope(ctx, async {
                runner
                    .run(EsRequest {
                        index:  "payments-2024.01".to_string(),
                        method: "POST".to_string(),
                        body:   json!({ "query": { "match_all": {} } }),
                    })
                    .await
                    .unwrap()
            })
            .await;

        assert_eq!(resp.total, 2);
    }

    #[tokio::test]
    async fn off_mode_executes_real_call_without_storing() {
        let store     = Arc::new(InMemoryStore::new());
        let record_id = Uuid::new_v4();
        let runner    = build_runner(store.clone() as Arc<dyn InteractionStore>);
        let ctx       = MockContext::with_id(record_id, ReplayMode::Off);

        MOCK_CTX
            .scope(ctx, async {
                let resp = runner
                    .run(EsRequest {
                        index:  "payments-current".to_string(),
                        method: "POST".to_string(),
                        body:   json!({}),
                    })
                    .await
                    .unwrap();
                assert_eq!(resp.total, 2, "Off mode must still return real data");
            })
            .await;

        let interactions = store.get_by_record_id(record_id).await.unwrap();
        assert!(interactions.is_empty(), "Off mode must not write to store");
    }
}
