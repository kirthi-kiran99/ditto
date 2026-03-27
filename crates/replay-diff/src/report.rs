//! Interaction-level diff report.
//!
//! [`InteractionDiffReport`] compares two sequences of [`Interaction`]s
//! (recorded vs replayed) and produces a structured report suitable for CI
//! output and PR comments.
//!
//! The JSON response diff for each matched pair is handled by [`DiffEngine`].
//! Structural issues (missing or extra calls) are flagged separately.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use replay_core::Interaction;

use crate::diff::{DiffEngine, DiffReport};
use crate::manifest::ChangeManifest;

// ── InteractionMismatch ───────────────────────────────────────────────────────

/// One mismatch between a recorded and replayed interaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionMismatch {
    /// Sequence number in the recording.
    pub sequence: u32,
    /// Normalized call fingerprint.
    pub fingerprint: String,
    /// Field-level diff for this interaction.
    pub diff: DiffReport,
}

// ── InteractionDiffReport ─────────────────────────────────────────────────────

/// Full diff report for one replay run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionDiffReport {
    pub record_id: Uuid,
    /// `true` if any interaction has a regression.
    pub is_regression: bool,
    /// Number of recorded interactions that were successfully matched.
    pub matched: usize,
    /// Interactions in the recording that had no replay counterpart.
    pub missing_replays: Vec<u32>,
    /// Interactions in the replay that had no recording counterpart.
    pub extra_replays: Vec<u32>,
    /// Field-level diffs for each matched interaction pair.
    pub mismatches: Vec<InteractionMismatch>,
}

impl InteractionDiffReport {
    /// Compare recorded interactions against replayed interactions.
    ///
    /// Interactions are matched by sequence number. The response JSON of each
    /// matched pair is diffed using [`DiffEngine`] with default noise rules.
    pub fn compare(
        record_id: Uuid,
        recorded: &[Interaction],
        replayed: &[Interaction],
    ) -> Self {
        Self::compare_with_manifest(record_id, recorded, replayed, &ChangeManifest::default())
    }

    /// Like [`compare`][Self::compare] but applies a [`ChangeManifest`] to
    /// reclassify declared intentional changes.
    pub fn compare_with_manifest(
        record_id:  Uuid,
        recorded:   &[Interaction],
        replayed:   &[Interaction],
        manifest:   &ChangeManifest,
    ) -> Self {
        let engine = DiffEngine::new();

        // Index replayed by sequence for O(1) lookup
        let replayed_by_seq: std::collections::HashMap<u32, &Interaction> =
            replayed.iter().map(|i| (i.sequence, i)).collect();

        let mut mismatches       = Vec::new();
        let mut missing_replays  = Vec::new();
        let mut matched          = 0usize;

        for rec in recorded {
            match replayed_by_seq.get(&rec.sequence) {
                Some(rep) => {
                    matched += 1;
                    let mut diff = engine.compare(&rec.response, &rep.response);
                    diff.apply_manifest(manifest);
                    if !diff.nodes.is_empty() {
                        mismatches.push(InteractionMismatch {
                            sequence:    rec.sequence,
                            fingerprint: rec.fingerprint.clone(),
                            diff,
                        });
                    }
                }
                None => missing_replays.push(rec.sequence),
            }
        }

        let recorded_seqs: std::collections::HashSet<u32> =
            recorded.iter().map(|i| i.sequence).collect();
        let extra_replays: Vec<u32> = replayed
            .iter()
            .filter(|i| !recorded_seqs.contains(&i.sequence))
            .map(|i| i.sequence)
            .collect();

        let is_regression = mismatches.iter().any(|m| m.diff.is_regression)
            || !missing_replays.is_empty();

        Self {
            record_id,
            is_regression,
            matched,
            missing_replays,
            extra_replays,
            mismatches,
        }
    }

    /// Exit code for CI: 0 = pass, 1 = regression.
    pub fn exit_code(&self) -> i32 {
        if self.is_regression { 1 } else { 0 }
    }

    /// Machine-readable JSON for CI artifact upload or PR comment.
    pub fn to_json_value(&self) -> Value {
        json!({
            "record_id":       self.record_id,
            "is_regression":   self.is_regression,
            "matched":         self.matched,
            "missing_replays": self.missing_replays,
            "extra_replays":   self.extra_replays,
            "mismatches":      self.mismatches.iter().map(|m| json!({
                "sequence":    m.sequence,
                "fingerprint": m.fingerprint,
                "regression":  m.diff.is_regression,
                "summary":     m.diff.summary(),
                "nodes":       m.diff.nodes.iter().map(|n| json!({
                    "path":     n.path,
                    "old":      n.old_value,
                    "new":      n.new_value,
                    "category": format!("{:?}", n.category).to_lowercase(),
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    }

    /// Pretty-printed JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.to_json_value()).unwrap_or_default()
    }

    /// Human-readable text summary.
    pub fn summary(&self) -> String {
        if !self.is_regression {
            format!(
                "PASS  record_id={}  matched={}  extra={} missing={}",
                self.record_id,
                self.matched,
                self.extra_replays.len(),
                self.missing_replays.len(),
            )
        } else {
            let reg_count = self.mismatches.iter().filter(|m| m.diff.is_regression).count();
            let mut s = format!(
                "FAIL  record_id={}  regressions={}  missing={}\n",
                self.record_id,
                reg_count,
                self.missing_replays.len(),
            );
            for m in &self.mismatches {
                if m.diff.is_regression {
                    s.push_str(&format!("  seq={} [{}]\n", m.sequence, m.fingerprint));
                    s.push_str(&format!("    {}\n", m.diff.summary().replace('\n', "\n    ")));
                }
            }
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use replay_core::{CallType, Interaction};
    use serde_json::json;
    use uuid::Uuid;

    fn make_interaction(record_id: Uuid, seq: u32, response: Value) -> Interaction {
        Interaction::new(
            record_id, seq, CallType::Http,
            "GET /payments/{id}".to_string(),
            json!({"method": "GET"}),
            response, 10,
        )
    }

    #[test]
    fn identical_interactions_pass() {
        let id  = Uuid::new_v4();
        let rec = vec![make_interaction(id, 0, json!({"status": "ok"}))];
        let rep = vec![make_interaction(id, 0, json!({"status": "ok"}))];
        let r = InteractionDiffReport::compare(id, &rec, &rep);
        assert!(!r.is_regression);
        assert_eq!(r.matched, 1);
    }

    #[test]
    fn changed_status_is_regression() {
        let id  = Uuid::new_v4();
        let rec = vec![make_interaction(id, 0, json!({"status": "succeeded"}))];
        let rep = vec![make_interaction(id, 0, json!({"status": "failed"}))];
        let r = InteractionDiffReport::compare(id, &rec, &rep);
        assert!(r.is_regression);
        assert_eq!(r.exit_code(), 1);
    }

    #[test]
    fn missing_replay_is_regression() {
        let id  = Uuid::new_v4();
        let rec = vec![
            make_interaction(id, 0, json!({"status": "ok"})),
            make_interaction(id, 1, json!({"status": "ok"})),
        ];
        let rep = vec![make_interaction(id, 0, json!({"status": "ok"}))];
        let r = InteractionDiffReport::compare(id, &rec, &rep);
        assert!(r.is_regression);
        assert_eq!(r.missing_replays, vec![1]);
    }

    #[test]
    fn extra_replay_is_flagged_but_not_regression() {
        let id  = Uuid::new_v4();
        let rec = vec![make_interaction(id, 0, json!({"status": "ok"}))];
        let rep = vec![
            make_interaction(id, 0, json!({"status": "ok"})),
            make_interaction(id, 1, json!({"status": "ok"})),
        ];
        let r = InteractionDiffReport::compare(id, &rec, &rep);
        assert!(!r.is_regression);
        assert_eq!(r.extra_replays, vec![1]);
    }

    #[test]
    fn manifest_prevents_regression() {
        let id  = Uuid::new_v4();
        let rec = vec![make_interaction(id, 0, json!({"error": {"code": 1001}}))];
        let rep = vec![make_interaction(id, 0, json!({"error": {"code": "ERR_1001"}}))];

        let manifest = ChangeManifest::from_toml(
            r#"version = 1
[[intentional_changes]]
description = "code type change"
pattern = "error.code"
"#,
        );
        let r = InteractionDiffReport::compare_with_manifest(id, &rec, &rep, &manifest);
        assert!(!r.is_regression);
    }

    #[test]
    fn to_json_is_valid_json() {
        let id = Uuid::new_v4();
        let rec = vec![make_interaction(id, 0, json!({"status": "ok"}))];
        let rep = vec![make_interaction(id, 0, json!({"status": "failed"}))];
        let r = InteractionDiffReport::compare(id, &rec, &rep);
        let json_str = r.to_json();
        let parsed: Value = serde_json::from_str(&json_str).expect("valid json");
        assert_eq!(parsed["is_regression"], true);
    }
}
