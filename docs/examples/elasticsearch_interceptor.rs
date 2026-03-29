/// Example: Elasticsearch custom interceptor
///
/// Demonstrates how to implement [`ReplayInterceptor`] for a client library
/// that is not covered by the built-in interceptors in `replay-interceptors`.
///
/// Copy this file as your starting point when adding a new client library.
///
/// # Quick start
/// 1. Implement `ReplayInterceptor` for your client wrapper (see below)
/// 2. Create an [`InterceptorRunner`] with the interceptor + store
/// 3. Replace direct client calls with `runner.run(request).await`
///
/// # Cargo.toml additions
/// ```toml
/// [dependencies]
/// elasticsearch = "8"
/// replay-core  = { path = "../../crates/replay-core" }
///
/// [dev-dependencies]
/// replay-store = { path = "../../crates/replay-store" }
/// ```

// NOTE: This file is intentionally *not* in a compiled crate — it is a
//       template. Uncomment the `use` statements and fill in the blanks.
//
// use std::sync::Arc;
// use async_trait::async_trait;
// use elasticsearch::{Elasticsearch, SearchParts, http::transport::Transport};
// use replay_core::{
//     CallType, FingerprintBuilder, InterceptorRunner, InteractionStore,
//     ReplayError, ReplayInterceptor,
// };
// use serde::{Deserialize, Serialize};
// use serde_json::{json, Value};

// ── request / response types ──────────────────────────────────────────────────

/// Normalised ES search request (volatile fields stripped).
pub struct EsRequest {
    pub index:  String,
    pub method: String,
    pub body:   Value,
}

/// ES search response with timing / shard info stripped.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EsResponse {
    pub hits: Value,
    // `took` and `_shards` are intentionally excluded — they are volatile and
    // would cause spurious diff failures. Add them back only if your diff rules
    // explicitly allow them.
}

// ── error type ────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum EsInterceptorError {
    // Bridge ReplayError into this type so InterceptorRunner can surface
    // store misses and deserialization failures without an extra channel.
    #[error(transparent)]
    Replay(#[from] replay_core::ReplayError),

    // Wrap the real ES client error.
    // #[error("elasticsearch error: {0}")]
    // Client(#[from] elasticsearch::Error),
    #[error("elasticsearch error: {0}")]
    Client(String),
}

// ── interceptor ───────────────────────────────────────────────────────────────

pub struct EsInterceptor {
    // client: Elasticsearch,
}

// #[async_trait]
// impl ReplayInterceptor for EsInterceptor {
//     type Request  = EsRequest;
//     type Response = EsResponse;
//     type Error    = EsInterceptorError;
//
//     fn call_type(&self) -> CallType { CallType::Http }
//
//     fn fingerprint(&self, req: &EsRequest) -> String {
//         // Normalise the index name: strip numeric suffixes / date patterns.
//         // "payments-2024.01" and "payments-2024.02" should produce the same
//         // fingerprint so recordings stay valid across index rollovers.
//         let index = normalize_index(&req.index);
//         FingerprintBuilder::http(&req.method, &format!("/{index}/_search"))
//     }
//
//     fn normalize_request(&self, req: &EsRequest) -> Value {
//         let mut body = req.body.clone();
//         // Strip volatile fields that change per-request.
//         strip_keys(&mut body, &["scroll_id", "preference", "routing"]);
//         json!({
//             "method": req.method,
//             "index":  normalize_index(&req.index),
//             "body":   body,
//         })
//     }
//
//     fn normalize_response(&self, res: &EsResponse) -> Value {
//         // EsResponse already excludes `took` and `_shards` — just serialize.
//         serde_json::to_value(res).unwrap_or(Value::Null)
//     }
//
//     async fn execute(&self, req: EsRequest) -> Result<EsResponse, EsInterceptorError> {
//         let resp = self.client
//             .search(SearchParts::Index(&[&req.index]))
//             .body(req.body)
//             .send()
//             .await
//             .map_err(EsInterceptorError::Client)?;
//
//         let raw: Value = resp.json().await.map_err(EsInterceptorError::Client)?;
//
//         // Strip volatile top-level fields before returning.
//         let hits = raw["hits"].clone();
//         Ok(EsResponse { hits })
//     }
// }

// ── helpers ───────────────────────────────────────────────────────────────────

/// Strip date suffixes and numeric IDs from index names.
///
/// Examples:
///   "payments-2024.01.15"  →  "payments-{date}"
///   "logs-prod-42"         →  "logs-prod-{id}"
fn normalize_index(index: &str) -> String {
    // Very naive approach — replace the last `-XXXX` segment if it looks like
    // a date or monotonic counter.  Adjust the regex to match your naming scheme.
    let parts: Vec<&str> = index.split('-').collect();
    if let Some(last) = parts.last() {
        let looks_like_id = last.chars().all(|c| c.is_ascii_alphanumeric() || c == '.');
        if looks_like_id && last.len() > 3 {
            let prefix = parts[..parts.len() - 1].join("-");
            return format!("{prefix}-{{id}}");
        }
    }
    index.to_string()
}

/// Remove listed keys from a JSON object in place.
fn strip_keys(value: &mut Value, keys: &[&str]) {
    if let Some(obj) = value.as_object_mut() {
        for key in keys {
            obj.remove(*key);
        }
    }
}

// ── wiring (in your application's main / startup) ─────────────────────────────

// fn build_es_runner(
//     client: Elasticsearch,
//     store:  Arc<dyn InteractionStore>,
// ) -> InterceptorRunner<EsInterceptor> {
//     InterceptorRunner::new(EsInterceptor { client }, store)
// }
//
// // Usage:
// let runner = build_es_runner(es_client, store.clone());
// let response: EsResponse = runner.run(EsRequest {
//     index:  "payments-2024.01".to_string(),
//     method: "POST".to_string(),
//     body:   json!({ "query": { "match_all": {} } }),
// }).await?;
