use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

// ── Recording list ─────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct RecordingsPage {
    pub items:  Vec<RecordingSummaryJson>,
    pub total:  usize,
    pub limit:  usize,
    pub offset: usize,
}

#[derive(Serialize)]
pub struct RecordingSummaryJson {
    pub record_id:         Uuid,
    pub method:            String,
    pub path:              String,
    pub status_code:       u16,
    pub recorded_at:       DateTime<Utc>,
    pub build_hash:        String,
    pub service_name:      String,
    pub tag:               String,
    pub interaction_count: u32,
}

// ── Recording detail ───────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct RecordingDetailJson {
    pub record_id:    Uuid,
    pub interactions: Vec<InteractionJson>,
}

#[derive(Serialize)]
pub struct InteractionJson {
    pub id:          Uuid,
    pub sequence:    u32,
    pub call_type:   String,
    pub fingerprint: String,
    pub request:     Value,
    pub response:    Value,
    pub duration_ms: u64,
    pub status:      String,
    pub error:       Option<String>,
}

// ── Replay result ──────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ReplayResultJson {
    pub record_id:       Uuid,
    pub status:          String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error:           Option<String>,
    pub is_regression:   bool,
    pub duration_ms:     u64,
    pub matched:         usize,
    pub missing_replays: Vec<u32>,
    pub extra_replays:   Vec<u32>,
    pub mismatches:      Vec<MismatchJson>,
}

#[derive(Serialize)]
pub struct MismatchJson {
    pub sequence:      u32,
    pub fingerprint:   String,
    pub is_regression: bool,
    pub nodes:         Vec<DiffNodeJson>,
}

#[derive(Serialize)]
pub struct DiffNodeJson {
    pub path:      String,
    pub old_value: Value,
    pub new_value: Value,
    pub category:  String,
}

// ── Tag groups ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct TagSummaryJson {
    pub service_name:      String,
    pub tag:               String,
    pub recording_count:   u32,
    pub interaction_count: u32,
    pub last_recorded_at:  DateTime<Utc>,
}

#[derive(Serialize)]
pub struct TagsPage {
    pub items:  Vec<TagSummaryJson>,
    pub total:  usize,
    pub limit:  usize,
    pub offset: usize,
}

// ── Run-all result ──────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct RunAllResultJson {
    pub service_name: String,
    pub tag:          String,
    pub total:        usize,
    pub passed:       usize,
    pub failed:       usize,
    pub errors:       usize,
    pub duration_ms:  u64,
    pub results:      Vec<ReplayResultJson>,
}

// ── Query params ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ListParams {
    #[serde(default = "default_limit")]
    pub limit:  usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize { 20 }

/// Query params for tag-specific endpoints: `?service_name=X&tag=Y`
#[derive(Deserialize)]
pub struct TagParams {
    #[serde(default)]
    pub service_name: String,
    pub tag:          String,
}

/// Query params for tag-specific endpoints with pagination.
#[derive(Deserialize)]
pub struct TagListParams {
    #[serde(default)]
    pub service_name: String,
    pub tag:          String,
    #[serde(default = "default_limit")]
    pub limit:        usize,
    #[serde(default)]
    pub offset:       usize,
}
