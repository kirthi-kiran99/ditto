use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use replay_core::{CallType, Interaction, MacroStore};
use serde_json::Value;
use uuid::Uuid;

use crate::store::{InteractionStore, RecordingSummary, Result, StoreError};

/// Thread-safe in-memory store for unit tests. No Postgres required.
#[derive(Clone, Default, Debug)]
pub struct InMemoryStore {
    data: Arc<Mutex<HashMap<Uuid, Vec<Interaction>>>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-populate the store with a set of interactions.
    ///
    /// Useful in replay tests: seed the store with a previous recording, then
    /// run the service under test in `ReplayMode::Replay` to verify it returns
    /// the stored responses.
    pub fn seed(&self, interactions: impl IntoIterator<Item = Interaction>) {
        let mut data = self.data.lock().expect("seed: lock poisoned");
        for i in interactions {
            data.entry(i.record_id).or_default().push(i);
        }
    }

    /// All interactions across every `record_id`, in insertion order.
    ///
    /// Primarily useful for post-request assertions in tests.
    pub fn all(&self) -> Vec<Interaction> {
        let data = self.data.lock().expect("all: lock poisoned");
        data.values().flat_map(|v| v.iter().cloned()).collect()
    }

    /// Total number of stored interactions across all `record_id`s.
    pub fn len(&self) -> usize {
        let data = self.data.lock().expect("len: lock poisoned");
        data.values().map(|v| v.len()).sum()
    }

    /// Returns `true` if no interactions have been stored yet.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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

    async fn delete_by_record_id(&self, record_id: Uuid) -> Result<()> {
        let mut data = self.data.lock().map_err(|e| StoreError::Database(e.to_string()))?;
        data.remove(&record_id);
        Ok(())
    }

    async fn get_recent_record_ids(&self, limit: usize) -> Result<Vec<Uuid>> {
        let data = self.data.lock().map_err(|e| StoreError::Database(e.to_string()))?;
        let ids: Vec<Uuid> = data.keys().copied().take(limit).collect();
        Ok(ids)
    }

    async fn list_recordings(
        &self,
        limit:  usize,
        offset: usize,
    ) -> Result<Vec<RecordingSummary>> {
        let data = self.data.lock().map_err(|e| StoreError::Database(e.to_string()))?;

        // Build a summary per record_id, sorted by recorded_at DESC.
        let mut summaries: Vec<RecordingSummary> = data
            .iter()
            .map(|(id, interactions)| {
                let count = interactions.len() as u32;
                let entry = interactions.iter().find(|i| i.sequence == 0);
                match entry {
                    Some(e) => RecordingSummary {
                        record_id:         *id,
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
                        build_hash:        e.build_hash.clone(),
                        service_name:      e.service_name.clone(),
                        interaction_count: count,
                    },
                    None => RecordingSummary {
                        record_id:         *id,
                        method:            String::new(),
                        path:              String::new(),
                        status_code:       0,
                        recorded_at:       chrono::Utc::now(),
                        build_hash:        String::new(),
                        service_name:      String::new(),
                        interaction_count: count,
                    },
                }
            })
            .collect();

        // Most recent first.
        summaries.sort_by(|a, b| b.recorded_at.cmp(&a.recorded_at));
        Ok(summaries.into_iter().skip(offset).take(limit).collect())
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
