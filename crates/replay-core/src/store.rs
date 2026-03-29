use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{CallType, Interaction};

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("interaction not found")]
    NotFound,
    #[error("database error: {0}")]
    Database(String),
}

/// Summary of a single recorded request — one row in the recordings list UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingSummary {
    pub record_id:         Uuid,
    /// HTTP method of the entry-point request (e.g. "GET", "POST").
    pub method:            String,
    /// URL path of the entry-point request (e.g. "/payments/42").
    pub path:              String,
    /// HTTP status code returned by the entry-point response.
    pub status_code:       u16,
    pub recorded_at:       DateTime<Utc>,
    pub build_hash:        String,
    pub service_name:      String,
    /// Total number of interactions (entry-point + all outbound calls).
    pub interaction_count: u32,
}

#[async_trait]
pub trait InteractionStore: Send + Sync {
    /// Append one interaction to the log.
    async fn write(&self, interaction: &Interaction) -> Result<()>;

    /// Append multiple interactions in one round-trip (preferred in production).
    async fn write_batch(&self, interactions: &[Interaction]) -> Result<()> {
        for i in interactions {
            self.write(i).await?;
        }
        Ok(())
    }

    /// All interactions for a request, ordered by sequence.
    async fn get_by_record_id(&self, record_id: Uuid) -> Result<Vec<Interaction>>;

    /// The entry-point interaction for a request (sequence == 0, call_type == Http).
    async fn get_entry_point(&self, record_id: Uuid) -> Result<Option<Interaction>> {
        let all = self.get_by_record_id(record_id).await?;
        Ok(all.into_iter().find(|i| i.sequence == 0))
    }

    /// Exact match: record_id + call_type + fingerprint + sequence.
    async fn find_match(
        &self,
        record_id:   Uuid,
        call_type:   CallType,
        fingerprint: &str,
        sequence:    u32,
    ) -> Result<Option<Interaction>>;

    /// Fuzzy match: same fingerprint, nearest sequence (handles insertion drift).
    async fn find_nearest(
        &self,
        record_id:   Uuid,
        fingerprint: &str,
        sequence:    u32,
    ) -> Result<Option<Interaction>>;

    /// Delete all interactions for a record_id (used to clean up shadow captures).
    async fn delete_by_record_id(&self, _record_id: Uuid) -> Result<()> {
        Ok(()) // default no-op; concrete stores override for real cleanup
    }

    /// Recent record IDs for the replay harness to iterate over.
    async fn get_recent_record_ids(&self, limit: usize) -> Result<Vec<Uuid>>;

    /// Paginated list of recordings for the replay UI.
    ///
    /// Each row contains the entry-point method/path/status and an aggregate
    /// interaction count so the UI can render the recordings list without
    /// loading all interactions up front.
    ///
    /// Ordered by `recorded_at` descending (most recent first).
    async fn list_recordings(
        &self,
        limit:  usize,
        offset: usize,
    ) -> Result<Vec<RecordingSummary>> {
        // Default: derive from existing methods (inefficient but always correct).
        // Concrete stores override this with a single aggregating query.
        let ids = self.get_recent_record_ids(limit + offset).await?;
        let ids = ids.into_iter().skip(offset).take(limit);
        let mut summaries = Vec::new();
        for id in ids {
            let interactions = self.get_by_record_id(id).await?;
            let count = interactions.len() as u32;
            let entry = interactions.into_iter().find(|i| i.sequence == 0);
            let summary = match entry {
                Some(e) => RecordingSummary {
                    record_id:         id,
                    method:            e.request.get("method")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("").to_string(),
                    path:              e.request.get("path")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("").to_string(),
                    status_code:       e.response.get("status")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0) as u16,
                    recorded_at:       e.recorded_at,
                    build_hash:        e.build_hash,
                    service_name:      e.service_name,
                    interaction_count: count,
                },
                None => RecordingSummary {
                    record_id:         id,
                    method:            String::new(),
                    path:              String::new(),
                    status_code:       0,
                    recorded_at:       Utc::now(),
                    build_hash:        String::new(),
                    service_name:      String::new(),
                    interaction_count: count,
                },
            };
            summaries.push(summary);
        }
        Ok(summaries)
    }
}
