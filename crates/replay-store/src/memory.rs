use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use replay_core::{CallType, Interaction, MacroStore};
use serde_json::Value;
use uuid::Uuid;

use crate::store::{InteractionStore, Result, StoreError};

/// Thread-safe in-memory store for unit tests. No Postgres required.
#[derive(Clone, Default, Debug)]
pub struct InMemoryStore {
    data: Arc<Mutex<HashMap<Uuid, Vec<Interaction>>>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl InteractionStore for InMemoryStore {
    async fn write(&self, interaction: &Interaction) -> Result<()> {
        let mut data = self.data.lock().map_err(|e| StoreError::Database(e.to_string()))?;
        data.entry(interaction.record_id).or_default().push(interaction.clone());
        Ok(())
    }

    async fn get_by_record_id(&self, record_id: Uuid) -> Result<Vec<Interaction>> {
        let data = self.data.lock().map_err(|e| StoreError::Database(e.to_string()))?;
        let mut interactions = data.get(&record_id).cloned().unwrap_or_default();
        interactions.sort_by_key(|i| i.sequence);
        Ok(interactions)
    }

    async fn find_match(
        &self,
        record_id:   Uuid,
        call_type:   CallType,
        fingerprint: &str,
        sequence:    u32,
    ) -> Result<Option<Interaction>> {
        let data = self.data.lock().map_err(|e| StoreError::Database(e.to_string()))?;
        let result = data
            .get(&record_id)
            .and_then(|interactions| {
                interactions.iter().find(|i| {
                    i.call_type == call_type
                        && i.fingerprint == fingerprint
                        && i.sequence == sequence
                })
            })
            .cloned();
        Ok(result)
    }

    async fn find_nearest(
        &self,
        record_id:   Uuid,
        fingerprint: &str,
        sequence:    u32,
    ) -> Result<Option<Interaction>> {
        let data = self.data.lock().map_err(|e| StoreError::Database(e.to_string()))?;
        let result = data
            .get(&record_id)
            .and_then(|interactions| {
                interactions
                    .iter()
                    .filter(|i| i.fingerprint == fingerprint)
                    .min_by_key(|i| (i.sequence as i32 - sequence as i32).unsigned_abs())
            })
            .cloned();
        Ok(result)
    }

    async fn get_recent_record_ids(&self, limit: usize) -> Result<Vec<Uuid>> {
        let data = self.data.lock().map_err(|e| StoreError::Database(e.to_string()))?;
        let ids: Vec<Uuid> = data.keys().copied().take(limit).collect();
        Ok(ids)
    }
}

// ── MacroStore impl ───────────────────────────────────────────────────────────

#[async_trait]
impl MacroStore for InMemoryStore {
    async fn store_fn_call(&self, interaction: &Interaction) {
        let _ = self.write(interaction).await;
    }

    async fn load_fn_response(
        &self,
        record_id:   Uuid,
        fingerprint: &str,
        sequence:    u32,
    ) -> Option<Value> {
        // Exact match first, then nearest
        let exact = self
            .find_match(record_id, replay_core::CallType::Function, fingerprint, sequence)
            .await
            .ok()
            .flatten();

        if exact.is_some() {
            return exact.map(|i| i.response);
        }

        self.find_nearest(record_id, fingerprint, sequence)
            .await
            .ok()
            .flatten()
            .map(|i| i.response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use replay_core::{CallStatus, CallType, Interaction};
    use serde_json::json;

    fn make_interaction(record_id: Uuid, sequence: u32, call_type: CallType, fp: &str) -> Interaction {
        Interaction::new(
            record_id,
            sequence,
            call_type,
            fp.to_string(),
            json!({}),
            json!({}),
            0,
        )
    }

    #[tokio::test]
    async fn write_and_read_back_ordered() {
        let store = InMemoryStore::new();
        let id = Uuid::new_v4();

        // Write out of order
        for seq in [4u32, 1, 3, 0, 2] {
            store.write(&make_interaction(id, seq, CallType::Http, "GET /foo")).await.unwrap();
        }

        let results = store.get_by_record_id(id).await.unwrap();
        assert_eq!(results.len(), 5);
        for (i, r) in results.iter().enumerate() {
            assert_eq!(r.sequence, i as u32, "should be sorted by sequence");
        }
    }

    #[tokio::test]
    async fn find_match_exact() {
        let store = InMemoryStore::new();
        let id = Uuid::new_v4();
        store.write(&make_interaction(id, 0, CallType::Http, "GET /payments")).await.unwrap();
        store.write(&make_interaction(id, 1, CallType::Postgres, "SELECT ?")).await.unwrap();

        let m = store.find_match(id, CallType::Http, "GET /payments", 0).await.unwrap();
        assert!(m.is_some());
        assert_eq!(m.unwrap().sequence, 0);

        let miss = store.find_match(id, CallType::Http, "GET /payments", 1).await.unwrap();
        assert!(miss.is_none(), "wrong sequence should not match");
    }

    #[tokio::test]
    async fn find_nearest_handles_drift() {
        let store = InMemoryStore::new();
        let id = Uuid::new_v4();
        // Recorded at sequence 2
        store.write(&make_interaction(id, 2, CallType::Http, "POST /charges")).await.unwrap();

        // New build has an extra call, so it asks for sequence 3
        let m = store.find_nearest(id, "POST /charges", 3).await.unwrap();
        assert!(m.is_some());
        assert_eq!(m.unwrap().sequence, 2, "nearest should bridge a drift of 1");
    }

    #[tokio::test]
    async fn find_nearest_returns_none_when_no_fingerprint_match() {
        let store = InMemoryStore::new();
        let id = Uuid::new_v4();
        store.write(&make_interaction(id, 0, CallType::Http, "GET /foo")).await.unwrap();

        let m = store.find_nearest(id, "GET /bar", 0).await.unwrap();
        assert!(m.is_none());
    }

    #[tokio::test]
    async fn empty_record_id_returns_empty_vec() {
        let store = InMemoryStore::new();
        let results = store.get_by_record_id(Uuid::new_v4()).await.unwrap();
        assert!(results.is_empty());
    }
}
