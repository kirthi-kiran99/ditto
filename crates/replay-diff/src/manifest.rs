//! PR change manifest — intentional diff declarations.
//!
//! A `.ditto-manifest.toml` committed alongside a PR lets developers declare
//! that certain field changes are expected. The diff engine reclassifies
//! matching [`DiffCategory::Regression`] nodes as
//! [`DiffCategory::Intentional`], preventing CI failures for known changes.
//!
//! # Example `.ditto-manifest.toml`
//! ```toml
//! version = 1
//!
//! [[intentional_changes]]
//! description = "error codes changed from numeric to string"
//! pattern     = "error.code"
//!
//! [[intentional_changes]]
//! description = "added new top-level currency field"
//! pattern     = "currency"
//! ```

use serde::{Deserialize, Serialize};

/// A loaded `.ditto-manifest.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChangeManifest {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub intentional_changes: Vec<IntentionalChange>,
}

/// One declared intentional change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentionalChange {
    /// Human description for the PR reviewer.
    pub description: String,
    /// Dot-separated field path pattern, e.g. `"response.error.code"`.
    /// Matches exact paths and any path whose terminal segment equals this value.
    pub pattern: String,
}

impl ChangeManifest {
    /// Parse a manifest from a TOML string.
    ///
    /// Returns a default (empty) manifest on parse failure rather than
    /// propagating errors — a missing or malformed manifest should not block CI.
    pub fn from_toml(content: &str) -> Self {
        toml::from_str(content).unwrap_or_default()
    }

    /// Load from a file path; returns an empty manifest if the file is absent.
    pub fn from_file(path: &std::path::Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(content) => Self::from_toml(&content),
            Err(_) => Self::default(),
        }
    }

    /// Returns `true` if `field_path` matches any declared intentional change.
    pub fn matches(&self, field_path: &str) -> bool {
        self.intentional_changes.iter().any(|c| {
            field_path == c.pattern
                || field_path.ends_with(&format!(".{}", c.pattern))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_manifest_matches_nothing() {
        let m = ChangeManifest::default();
        assert!(!m.matches("response.status"));
    }

    #[test]
    fn manifest_matches_exact_path() {
        let m = ChangeManifest::from_toml(
            r#"version = 1
[[intentional_changes]]
description = "test"
pattern = "response.error.code"
"#,
        );
        assert!(m.matches("response.error.code"));
        assert!(!m.matches("response.status"));
    }

    #[test]
    fn manifest_matches_terminal_segment() {
        let m = ChangeManifest::from_toml(
            r#"version = 1
[[intentional_changes]]
description = "currency added"
pattern = "currency"
"#,
        );
        // Matches as terminal segment of a longer path
        assert!(m.matches("data.payment.currency"));
        assert!(m.matches("currency"));
    }

    #[test]
    fn malformed_toml_returns_empty_manifest() {
        let m = ChangeManifest::from_toml("this is not valid toml }{");
        assert!(m.intentional_changes.is_empty());
    }

    #[test]
    fn apply_manifest_reclassifies_regressions() {
        use crate::diff::{DiffEngine};
        use serde_json::json;

        let old = json!({"error": {"code": 1001}, "status": "failed"});
        let new = json!({"error": {"code": "ERR_1001"}, "status": "failed"});
        let mut report = DiffEngine::new().compare(&old, &new);
        assert!(report.is_regression);

        let manifest = ChangeManifest::from_toml(
            r#"version = 1
[[intentional_changes]]
description = "error code type change"
pattern = "error.code"
"#,
        );
        report.apply_manifest(&manifest);
        assert!(!report.is_regression, "regression should be reclassified as intentional");
    }
}
