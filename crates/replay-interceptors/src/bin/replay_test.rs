//! `replay_test` — CLI harness for running ditto replay tests.
//!
//! Reads configuration from environment variables, fires recorded requests
//! at the service under test, diffs responses, and exits with an appropriate
//! code.
//!
//! # Environment variables
//!
//! | Variable              | Required | Description                                  |
//! |-----------------------|----------|----------------------------------------------|
//! | `REPLAY_DB_URL`       | yes      | Postgres DSN for the interaction store       |
//! | `REPLAY_TARGET_URL`   | yes      | Base URL of the service under test           |
//! | `REPLAY_SAMPLE_SIZE`  | no       | Number of recent recordings to replay (50)   |
//! | `REPLAY_RECORD_ID`    | no       | Replay a single specific recording           |
//! | `REPLAY_REPORT_PATH`  | no       | Write JSON report to this file               |
//! | `REPLAY_MANIFEST`     | no       | Path to `.ditto-manifest.toml`               |
//! | `REPLAY_TOKEN_MAP`    | no       | `prod_key:test_key,key2:val2` mappings       |
//!
//! # Exit codes
//! - `0` — all replays passed
//! - `1` — one or more regressions detected
//! - `2` — harness error (config, network, store)

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use replay_diff::manifest::ChangeManifest;
use replay_interceptors::harness::{HarnessConfig, HarnessStatus, ReplayHarness};
use replay_store::PostgresStore;
use replay_core::InteractionStore;
use serde_json::json;
use uuid::Uuid;

#[tokio::main]
async fn main() {
    let cfg = match Config::from_env() {
        Ok(c)  => c,
        Err(e) => {
            eprintln!("[ditto] error: {e}");
            std::process::exit(2);
        }
    };

    // ── connect to store ──────────────────────────────────────────────────────
    let store: Arc<dyn InteractionStore> = match PostgresStore::new(&cfg.db_url).await {
        Ok(s)  => Arc::new(s),
        Err(e) => {
            eprintln!("[ditto] failed to connect to store: {e}");
            std::process::exit(2);
        }
    };

    // ── collect record IDs ────────────────────────────────────────────────────
    let record_ids: Vec<Uuid> = if let Some(id) = cfg.single_record_id {
        vec![id]
    } else {
        match store.get_recent_record_ids(cfg.sample_size).await {
            Ok(ids) => ids,
            Err(e) => {
                eprintln!("[ditto] failed to load record IDs: {e}");
                std::process::exit(2);
            }
        }
    };

    if record_ids.is_empty() {
        println!("[ditto] no recordings found — nothing to replay");
        std::process::exit(0);
    }

    println!("[ditto] replaying {} recording(s) against {}", record_ids.len(), cfg.target_url);

    // ── run harness ───────────────────────────────────────────────────────────
    let harness_cfg = HarnessConfig {
        token_map:    cfg.token_map,
        manifest:     cfg.manifest,
        timeout_secs: 30,
    };
    let harness = ReplayHarness::with_config(store, cfg.target_url, harness_cfg);
    let results = harness.run_all(record_ids).await;

    // ── summarize ─────────────────────────────────────────────────────────────
    let passed   = results.iter().filter(|r| r.status == HarnessStatus::Passed).count();
    let failed   = results.iter().filter(|r| r.status == HarnessStatus::Failed).count();
    let errored  = results.iter().filter(|r| matches!(r.status, HarnessStatus::Error(_))).count();
    let total    = results.len();

    println!("[ditto] {passed}/{total} passed  ({failed} failed, {errored} errors)");

    for r in &results {
        match &r.status {
            HarnessStatus::Passed => {}
            HarnessStatus::Failed => println!("  FAIL {}", r.record_id),
            HarnessStatus::Error(e) => println!("  ERR  {} — {}", r.record_id, e),
        }
        if r.report.is_regression {
            for m in &r.report.mismatches {
                if m.diff.is_regression {
                    println!("       seq={} [{}]", m.sequence, m.fingerprint);
                    for node in m.diff.nodes.iter().filter(|n| matches!(n.category, replay_diff::DiffCategory::Regression)) {
                        println!("         {}: {} → {}", node.path, node.old_value, node.new_value);
                    }
                }
            }
        }
    }

    // ── write JSON report ─────────────────────────────────────────────────────
    if let Some(report_path) = cfg.report_path {
        let json_reports: Vec<serde_json::Value> = results
            .iter()
            .map(|r| r.report.to_json_value())
            .collect();
        let full_report = json!({
            "total":   total,
            "passed":  passed,
            "failed":  failed,
            "errored": errored,
            "results": json_reports,
        });
        if let Err(e) = std::fs::write(&report_path, serde_json::to_string_pretty(&full_report).unwrap_or_default()) {
            eprintln!("[ditto] failed to write report to {}: {e}", report_path.display());
        } else {
            println!("[ditto] report written to {}", report_path.display());
        }
    }

    std::process::exit(if failed > 0 || errored > 0 { 1 } else { 0 });
}

// ── Config ────────────────────────────────────────────────────────────────────

struct Config {
    db_url:           String,
    target_url:       String,
    sample_size:      usize,
    single_record_id: Option<Uuid>,
    report_path:      Option<PathBuf>,
    manifest:         ChangeManifest,
    token_map:        HashMap<String, String>,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let db_url = std::env::var("REPLAY_DB_URL")
            .map_err(|_| "REPLAY_DB_URL is required")?;
        let target_url = std::env::var("REPLAY_TARGET_URL")
            .map_err(|_| "REPLAY_TARGET_URL is required")?;

        let sample_size = std::env::var("REPLAY_SAMPLE_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(50);

        let single_record_id = std::env::var("REPLAY_RECORD_ID")
            .ok()
            .and_then(|v| Uuid::parse_str(&v).ok());

        let report_path = std::env::var("REPLAY_REPORT_PATH")
            .ok()
            .map(PathBuf::from);

        let manifest = std::env::var("REPLAY_MANIFEST")
            .ok()
            .map(|p| ChangeManifest::from_file(std::path::Path::new(&p)))
            .unwrap_or_default();

        let token_map = std::env::var("REPLAY_TOKEN_MAP")
            .ok()
            .map(|v| parse_token_map(&v))
            .unwrap_or_default();

        Ok(Self {
            db_url,
            target_url,
            sample_size,
            single_record_id,
            report_path,
            manifest,
            token_map,
        })
    }
}

/// Parse `"prod_key:test_key,key2:val2"` into a HashMap.
fn parse_token_map(s: &str) -> HashMap<String, String> {
    s.split(',')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, ':');
            let k = parts.next()?.trim().to_string();
            let v = parts.next()?.trim().to_string();
            Some((k, v))
        })
        .collect()
}
