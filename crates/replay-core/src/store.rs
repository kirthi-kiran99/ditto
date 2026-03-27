use async_trait::async_trait;
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

    /// Recent record IDs for the replay harness to iterate over.
    async fn get_recent_record_ids(&self, limit: usize) -> Result<Vec<Uuid>>;
}
