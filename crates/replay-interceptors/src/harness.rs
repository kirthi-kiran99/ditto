//! Replay harness — drives recorded HTTP entry-points against the service under
//! test and diffs the resulting interaction chains.
//!
//! # How replay works end-to-end
//!
//! ```text
//! ┌─────────────┐  ①  fire entry-point HTTP request  ┌───────────────────────┐
//! │   Harness   │ ─────────────────────────────────►  │  Service under test   │
//! │             │      X-Replay-Record-Id: <uuid>      │  (REPLAY_MODE=replay) │
//! │             │                                      │                       │
//! │             │  ②  service serves mock responses    │  recording_middleware │
//! │             │      from the store for all outbound │  sets up MockContext  │
//! │             │      calls (HTTP, DB, Redis)         │  using the header     │
//! │             │                                      └───────────────────────┘
//! │             │  ③  compare outer HTTP response
//! │             │      against recorded entry-point
//! └─────────────┘
//! ```
//!
//! # Usage
//! ```ignore
//! let store   = Arc::new(PostgresStore::new(&db_url).await?);
//! let harness = ReplayHarness::new(store, "http://localhost:8080".into());
//!
//! let record_ids = store.get_recent_record_ids(50).await?;
//! let results    = harness.run_all(record_ids).await;
//!
//! for r in &results {
//!     println!("{}", r.report.summary());
//! }
//! let failed = results.iter().filter(|r| r.report.is_regression).count();
//! std::process::exit(if failed > 0 { 1 } else { 0 });
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use replay_core::InteractionStore;
use replay_diff::report::InteractionDiffReport;
use replay_diff::manifest::ChangeManifest;
use serde_json::Value;
use uuid::Uuid;

// ── HarnessStatus ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessStatus {
    /// Replay matched the recording — no regressions.
    Passed,
    /// At least one response field regressed.
    Failed,
    /// The harness itself encountered an error (network, store, etc.).
    Error(String),
}

// ── ReplayResult ──────────────────────────────────────────────────────────────

/// Result of replaying one recorded request.
#[derive(Debug)]
pub struct ReplayResult {
    pub record_id:   Uuid,
    pub status:      HarnessStatus,
    pub report:      InteractionDiffReport,
    pub duration_ms: u64,
}

// ── HarnessConfig ─────────────────────────────────────────────────────────────

/// Configuration for [`ReplayHarness`].
#[derive(Debug, Clone)]
pub struct HarnessConfig {
    /// Auth-token translation map: `recorded_token → test_env_token`.
    /// The harness swaps Authorization header values before firing requests.
    pub token_map: HashMap<String, String>,
    /// `.ditto-manifest.toml` contents (empty = no intentional-change overrides).
    pub manifest: ChangeManifest,
    /// HTTP request timeout in seconds (default: 30).
    pub timeout_secs: u64,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            token_map:    HashMap::new(),
            manifest:     ChangeManifest::default(),
            timeout_secs: 30,
        }
    }
}

// ── ReplayHarness ─────────────────────────────────────────────────────────────

/// Drives recorded entry-points against a running service and diffs the results.
pub struct ReplayHarness {
    store:      Arc<dyn InteractionStore>,
    target_url: String,
    client:     reqwest::Client,
    config:     HarnessConfig,
}

impl ReplayHarness {
    pub fn new(store: Arc<dyn InteractionStore>, target_url: String) -> Self {
        Self::with_config(store, target_url, HarnessConfig::default())
    }

    pub fn with_config(
        store:      Arc<dyn InteractionStore>,
        target_url: String,
        config:     HarnessConfig,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .expect("reqwest client");
        Self { store, target_url, client, config }
    }

    /// Replay one recording and return the diff report.
    pub async fn run_one(&self, record_id: Uuid) -> ReplayResult {
        let start = Instant::now();

        // 1. Load all recorded interactions for this request
        let recorded = match self.store.get_by_record_id(record_id).await {
            Ok(v) => v,
            Err(e) => return self.error_result(record_id, format!("store read: {e}")),
        };

        if recorded.is_empty() {
            return self.error_result(record_id, "no recorded interactions".into());
        }

        // 2. Load entry-point (sequence=0) to know which endpoint to fire
        let entry = match self.store.get_entry_point(record_id).await {
            Ok(Some(e)) => e,
            Ok(None)    => return self.error_result(record_id, "no entry-point (sequence=0) found".into()),
            Err(e)      => return self.error_result(record_id, format!("entry-point load: {e}")),
        };

        let method = entry.request["method"].as_str().unwrap_or("GET").to_string();
        let path   = entry.request["path"].as_str().unwrap_or("/").to_string();
        let url    = format!("{}{}", self.target_url.trim_end_matches('/'), path);

        // 3. Build and fire the request
        let mut builder = self.client
            .request(
                method.parse().unwrap_or(reqwest::Method::GET),
                &url,
            )
            .header("x-replay-record-id", record_id.to_string())
            .header("content-type", "application/json");

        // Apply auth token translation
        if let Some(auth) = entry.request["headers"]["authorization"].as_str() {
            let translated = self.translate_token(auth);
            builder = builder.header("authorization", translated);
        }

        // Fire request
        let resp = match builder.send().await {
            Ok(r)  => r,
            Err(e) => return self.error_result(record_id, format!("HTTP request failed: {e}")),
        };

        let replay_status = resp.status().as_u16();
        let replay_body   = resp.json::<Value>().await.unwrap_or(Value::Null);

        // 4. Build a synthetic "replayed" interaction list for the entry-point comparison.
        //
        //    The outer HTTP response (status + body) is the primary comparison target.
        //    We update only the entry-point in a clone; all other interactions keep
        //    their recorded values (inner calls are not re-captured during the
        //    lightweight harness run — that would require a full replay store write-back).
        let mut replayed_interactions = recorded.clone();
        if let Some(ep) = replayed_interactions.iter_mut().find(|i| i.sequence == 0) {
            // Build replayed response with only fields the recording actually had,
            // plus the body if the service returned one.
            // Use the recorded status as the baseline so we don't flag noise fields;
            // the status comparison happens via the diff engine.
            let mut resp = serde_json::Map::new();
            resp.insert("status".to_string(), serde_json::json!(replay_status));
            if !replay_body.is_null() {
                resp.insert("body".to_string(), replay_body);
            }
            ep.response = Value::Object(resp);
        }

        // The recorded interactions are compared as-is.  If the recording has
        // no "body" field in the entry-point response, the diff engine will see
        // any new "body" as DiffCategory::Added (not Regression).
        let recorded_for_diff = recorded;

        // 5. Diff
        let report = InteractionDiffReport::compare_with_manifest(
            record_id,
            &recorded_for_diff,
            &replayed_interactions,
            &self.config.manifest,
        );

        let status = if matches!(report.is_regression, true) {
            HarnessStatus::Failed
        } else {
            HarnessStatus::Passed
        };

        ReplayResult {
            record_id,
            status,
            report,
            duration_ms: start.elapsed().as_millis() as u64,
        }
    }

    /// Replay all provided record IDs concurrently and return results.
    pub async fn run_all(&self, record_ids: Vec<Uuid>) -> Vec<ReplayResult> {
        let futs: Vec<_> = record_ids
            .into_iter()
            .map(|id| self.run_one(id))
            .collect();
        futures_util::future::join_all(futs).await
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    fn translate_token(&self, token: &str) -> String {
        let bare = token.trim_start_matches("Bearer ").trim();
        if let Some(mapped) = self.config.token_map.get(bare) {
            format!("Bearer {}", mapped)
        } else {
            token.to_string()
        }
    }

    fn error_result(&self, record_id: Uuid, msg: String) -> ReplayResult {
        // Build a minimal "failed" report with the error as a mismatch
        let dummy_report = InteractionDiffReport {
            record_id,
            is_regression:  true,
            matched:        0,
            missing_replays: vec![],
            extra_replays:  vec![],
            mismatches:     vec![],
        };
        ReplayResult {
            record_id,
            status:      HarnessStatus::Error(msg),
            report:      dummy_report,
            duration_ms: 0,
        }
    }
}
