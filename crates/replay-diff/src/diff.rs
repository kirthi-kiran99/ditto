//! Recursive JSON diff with noise classification.
//!
//! The engine walks two `serde_json::Value` trees simultaneously and emits a
//! [`DiffNode`] for every difference. Each node is classified as [`DiffCategory::Noise`]
//! (expected volatility), [`DiffCategory::Regression`] (unexpected change), or
//! [`DiffCategory::Added`] / [`DiffCategory::Removed`] (structural change).
//!
//! # Noise auto-detection
//! - Both values look like UUIDs → noise
//! - Both values look like ISO-8601 timestamps → noise
//! - The terminal field name matches a [`NoiseRule`] → noise
//!
//! # Usage
//! ```
//! use replay_diff::diff::{DiffEngine, DiffReport};
//! use serde_json::json;
//!
//! let recorded = json!({"status": "pending", "id": "550e8400-e29b-41d4-a716-446655440000"});
//! let actual   = json!({"status": "failed",  "id": "6ba7b810-9dad-11d1-80b4-00c04fd430c8"});
//! let report = DiffEngine::new().compare(&recorded, &actual);
//! assert!(report.is_regression);  // status changed
//! ```

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Noise rules ───────────────────────────────────────────────────────────────

/// What to do when a field name matches a noise rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoiseStrategy {
    /// Suppress the diff entirely — the field is always volatile.
    Ignore,
    /// Field contains a UUID — compare structure, not value.
    UuidNormalize,
    /// Field contains a timestamp — always suppress.
    TimestampIgnore,
}

/// A rule that suppresses diff noise for a named field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoiseRule {
    /// Terminal field name to match, e.g. `"id"`, `"created_at"`.
    /// Matches any path whose last segment equals this name.
    pub field_name: String,
    pub strategy: NoiseStrategy,
}

impl NoiseRule {
    fn new(field_name: &str, strategy: NoiseStrategy) -> Self {
        Self { field_name: field_name.to_string(), strategy }
    }
}

// ── DiffConfig ────────────────────────────────────────────────────────────────

/// Configuration for the diff engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffConfig {
    pub noise_rules: Vec<NoiseRule>,
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self {
            noise_rules: vec![
                // Identity fields — UUIDs that change every run
                NoiseRule::new("id",           NoiseStrategy::UuidNormalize),
                NoiseRule::new("request_id",   NoiseStrategy::Ignore),
                NoiseRule::new("trace_id",     NoiseStrategy::Ignore),
                NoiseRule::new("span_id",      NoiseStrategy::Ignore),
                NoiseRule::new("attempt_id",   NoiseStrategy::UuidNormalize),
                NoiseRule::new("idempotency_key", NoiseStrategy::Ignore),
                // Timestamps
                NoiseRule::new("created_at",   NoiseStrategy::TimestampIgnore),
                NoiseRule::new("updated_at",   NoiseStrategy::TimestampIgnore),
                NoiseRule::new("recorded_at",  NoiseStrategy::TimestampIgnore),
                NoiseRule::new("expires_at",   NoiseStrategy::TimestampIgnore),
                NoiseRule::new("timestamp",    NoiseStrategy::TimestampIgnore),
                // HTTP headers that are always different
                NoiseRule::new("x-request-id", NoiseStrategy::Ignore),
                NoiseRule::new("x-trace-id",   NoiseStrategy::Ignore),
                NoiseRule::new("date",         NoiseStrategy::TimestampIgnore),
            ],
        }
    }
}

// ── DiffCategory ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffCategory {
    /// Known volatile field — suppressed from regression counts.
    Noise,
    /// Declared intentional in a [`ChangeManifest`] — flagged but not failing.
    Intentional,
    /// Unexpected value change — fails CI.
    Regression,
    /// Field present in actual but not in recorded.
    Added,
    /// Field present in recorded but missing in actual.
    Removed,
}

// ── DiffNode ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffNode {
    /// JSON path to this field, e.g. `"response.body.status"`.
    pub path: String,
    pub old_value: Value,
    pub new_value: Value,
    pub category: DiffCategory,
}

// ── DiffReport ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffReport {
    pub nodes: Vec<DiffNode>,
    /// `true` if any node has category [`DiffCategory::Regression`].
    pub is_regression: bool,
}

impl DiffReport {
    /// Reclassify Regression nodes whose path matches a [`ChangeManifest`] pattern
    /// as [`DiffCategory::Intentional`]. Updates `is_regression` accordingly.
    pub fn apply_manifest(&mut self, manifest: &crate::manifest::ChangeManifest) {
        for node in &mut self.nodes {
            if matches!(node.category, DiffCategory::Regression)
                && manifest.matches(&node.path)
            {
                node.category = DiffCategory::Intentional;
            }
        }
        self.is_regression = self
            .nodes
            .iter()
            .any(|n| matches!(n.category, DiffCategory::Regression));
    }

    /// Serialize to pretty-printed JSON for CI artifact upload.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Human-readable one-line summary.
    pub fn summary(&self) -> String {
        let regressions = self.nodes.iter().filter(|n| matches!(n.category, DiffCategory::Regression)).count();
        let intentional = self.nodes.iter().filter(|n| matches!(n.category, DiffCategory::Intentional)).count();
        let noise       = self.nodes.iter().filter(|n| matches!(n.category, DiffCategory::Noise)).count();
        let added       = self.nodes.iter().filter(|n| matches!(n.category, DiffCategory::Added)).count();
        let removed     = self.nodes.iter().filter(|n| matches!(n.category, DiffCategory::Removed)).count();

        if !self.is_regression {
            format!(
                "PASS — {noise} noise, {intentional} intentional, {added} added, {removed} removed"
            )
        } else {
            let mut s = format!("FAIL — {regressions} regression(s):\n");
            for node in self.nodes.iter().filter(|n| matches!(n.category, DiffCategory::Regression)) {
                s.push_str(&format!(
                    "  {}: {} → {}\n",
                    node.path,
                    node.old_value,
                    node.new_value
                ));
            }
            s
        }
    }
}

// ── DiffEngine ────────────────────────────────────────────────────────────────

/// Stateless diff engine. Create once, call [`compare`][DiffEngine::compare] many times.
#[derive(Debug, Clone, Default)]
pub struct DiffEngine {
    config: DiffConfig,
}

impl DiffEngine {
    /// Create with default noise rules.
    pub fn new() -> Self {
        Self { config: DiffConfig::default() }
    }

    /// Create with custom configuration.
    pub fn with_config(config: DiffConfig) -> Self {
        Self { config }
    }

    /// Compare two JSON values and return a diff report.
    pub fn compare(&self, recorded: &Value, actual: &Value) -> DiffReport {
        let mut nodes = Vec::new();
        self.diff_recursive(recorded, actual, "", &mut nodes);
        let is_regression = nodes.iter().any(|n| matches!(n.category, DiffCategory::Regression));
        DiffReport { nodes, is_regression }
    }

    // ── recursive walker ──────────────────────────────────────────────────────

    fn diff_recursive(&self, old: &Value, new: &Value, path: &str, out: &mut Vec<DiffNode>) {
        match (old, new) {
            (Value::Object(o), Value::Object(n)) => {
                for (k, v) in o {
                    let child = child_path(path, k);
                    match n.get(k) {
                        Some(nv) => self.diff_recursive(v, nv, &child, out),
                        None     => out.push(DiffNode {
                            path: child,
                            old_value: v.clone(),
                            new_value: Value::Null,
                            category: DiffCategory::Removed,
                        }),
                    }
                }
                for (k, v) in n {
                    if !o.contains_key(k) {
                        out.push(DiffNode {
                            path: child_path(path, k),
                            old_value: Value::Null,
                            new_value: v.clone(),
                            category: DiffCategory::Added,
                        });
                    }
                }
            }

            (Value::Array(o), Value::Array(n)) => {
                let max = o.len().max(n.len());
                for i in 0..max {
                    let child = format!("{}[{}]", path, i);
                    match (o.get(i), n.get(i)) {
                        (Some(ov), Some(nv)) => self.diff_recursive(ov, nv, &child, out),
                        (Some(ov), None)     => out.push(DiffNode {
                            path: child,
                            old_value: ov.clone(),
                            new_value: Value::Null,
                            category: DiffCategory::Removed,
                        }),
                        (None, Some(nv))     => out.push(DiffNode {
                            path: child,
                            old_value: Value::Null,
                            new_value: nv.clone(),
                            category: DiffCategory::Added,
                        }),
                        _ => {}
                    }
                }
            }

            _ => {
                if old != new {
                    let category = self.classify(path, old, new);
                    out.push(DiffNode {
                        path: path.to_string(),
                        old_value: old.clone(),
                        new_value: new.clone(),
                        category,
                    });
                }
            }
        }
    }

    fn classify(&self, path: &str, old: &Value, new: &Value) -> DiffCategory {
        // Named noise rules take precedence
        for rule in &self.config.noise_rules {
            if path_matches_field(path, &rule.field_name) {
                return DiffCategory::Noise;
            }
        }
        // Auto-detect: both look like UUIDs → noise
        if is_uuid_value(old) && is_uuid_value(new) {
            return DiffCategory::Noise;
        }
        // Auto-detect: both look like timestamps → noise
        if is_timestamp_value(old) && is_timestamp_value(new) {
            return DiffCategory::Noise;
        }
        DiffCategory::Regression
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn child_path(parent: &str, key: &str) -> String {
    if parent.is_empty() {
        key.to_string()
    } else {
        format!("{}.{}", parent, key)
    }
}

/// Returns `true` if the terminal segment of `path` equals `field_name`.
fn path_matches_field(path: &str, field_name: &str) -> bool {
    path == field_name
        || path.ends_with(&format!(".{}", field_name))
}

fn uuid_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
            .unwrap()
    })
}

fn timestamp_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Matches ISO-8601: 2024-01-01T00:00:00Z  or  2024-01-01T00:00:00.000Z  etc.
        Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}").unwrap()
    })
}

fn is_uuid_value(v: &Value) -> bool {
    v.as_str().map(|s| uuid_re().is_match(s)).unwrap_or(false)
}

fn is_timestamp_value(v: &Value) -> bool {
    v.as_str().map(|s| timestamp_re().is_match(s)).unwrap_or(false)
}

// ── unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identical_values_produce_no_diff() {
        let v = json!({"status": "ok", "amount": 100});
        let r = DiffEngine::new().compare(&v, &v);
        assert!(r.nodes.is_empty());
        assert!(!r.is_regression);
    }

    #[test]
    fn changed_status_is_regression() {
        let old = json!({"status": "pending"});
        let new = json!({"status": "failed"});
        let r = DiffEngine::new().compare(&old, &new);
        assert!(r.is_regression);
        assert_eq!(r.nodes[0].path, "status");
        assert!(matches!(r.nodes[0].category, DiffCategory::Regression));
    }

    #[test]
    fn uuid_fields_are_noise() {
        let old = json!({"id": "550e8400-e29b-41d4-a716-446655440000", "status": "ok"});
        let new = json!({"id": "6ba7b810-9dad-11d1-80b4-00c04fd430c8", "status": "ok"});
        let r = DiffEngine::new().compare(&old, &new);
        assert!(!r.is_regression);
        assert!(r.nodes.iter().all(|n| matches!(n.category, DiffCategory::Noise)));
    }

    #[test]
    fn timestamp_fields_are_noise() {
        let old = json!({"created_at": "2024-01-01T00:00:00Z"});
        let new = json!({"created_at": "2024-06-15T12:34:56.789Z"});
        let r = DiffEngine::new().compare(&old, &new);
        assert!(!r.is_regression);
    }

    #[test]
    fn added_field_is_not_regression() {
        let old = json!({"status": "ok"});
        let new = json!({"status": "ok", "new_field": "value"});
        let r = DiffEngine::new().compare(&old, &new);
        assert!(!r.is_regression);
        assert!(r.nodes.iter().any(|n| matches!(n.category, DiffCategory::Added)));
    }

    #[test]
    fn removed_field_is_not_regression() {
        let old = json!({"status": "ok", "old_field": "value"});
        let new = json!({"status": "ok"});
        let r = DiffEngine::new().compare(&old, &new);
        assert!(!r.is_regression);
        assert!(r.nodes.iter().any(|n| matches!(n.category, DiffCategory::Removed)));
    }

    #[test]
    fn nested_regression_detected() {
        let old = json!({"data": {"payment": {"status": "succeeded"}}});
        let new = json!({"data": {"payment": {"status": "failed"}}});
        let r = DiffEngine::new().compare(&old, &new);
        assert!(r.is_regression);
        assert_eq!(r.nodes[0].path, "data.payment.status");
    }

    #[test]
    fn array_element_regression_detected() {
        let old = json!({"items": [{"amount": 100}, {"amount": 200}]});
        let new = json!({"items": [{"amount": 100}, {"amount": 999}]});
        let r = DiffEngine::new().compare(&old, &new);
        assert!(r.is_regression);
        assert_eq!(r.nodes[0].path, "items[1].amount");
    }

    #[test]
    fn named_noise_rule_suppresses_diff() {
        // "request_id" is in default noise rules
        let old = json!({"request_id": "abc", "status": "ok"});
        let new = json!({"request_id": "xyz", "status": "ok"});
        let r = DiffEngine::new().compare(&old, &new);
        assert!(!r.is_regression);
    }

    #[test]
    fn summary_shows_pass_for_no_regression() {
        let v = json!({"status": "ok"});
        let r = DiffEngine::new().compare(&v, &v);
        assert!(r.summary().starts_with("PASS"));
    }

    #[test]
    fn summary_shows_fail_and_path_for_regression() {
        let old = json!({"status": "pending"});
        let new = json!({"status": "failed"});
        let r = DiffEngine::new().compare(&old, &new);
        let s = r.summary();
        assert!(s.starts_with("FAIL"));
        assert!(s.contains("status"));
    }
}
