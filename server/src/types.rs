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

// ── Query params ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ListParams {
    #[serde(default = "default_limit")]
    pub limit:  usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize { 20 }
