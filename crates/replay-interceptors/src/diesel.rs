//! Diesel interceptor for ditto replay system.
//!
//! Provides record/replay support for Diesel async queries.

use std::sync::Arc;

use diesel::{
    pg::Pg,
    query_builder::QueryFragment,
    QueryResult,
};
use replay_core::{CallStatus, CallType, FingerprintBuilder, Interaction, ReplayMode};
use replay_store::InteractionStore;
use serde_json::{json, Value};
use uuid::Uuid;

// ── DieselReplayExecutor ──────────────────────────────────────────────────────

/// Wraps a store and provides replay instrumentation for Diesel queries.
///
/// In **record** mode: executes real queries and stores results.
/// In **replay** mode: returns stored results without touching the database.
#[derive(Clone)]
pub struct DieselReplayExecutor {
    store: Arc<dyn InteractionStore>,
}

impl DieselReplayExecutor {
    pub fn new(store: Arc<dyn InteractionStore>) -> Self {
        Self { store }
    }

    /// Record or replay an INSERT/UPDATE/DELETE operation.
    ///
    /// In replay mode, returns a fake success result based on stored data.
    pub async fn execute_insert(
        &self,
        record_id: uuid::Uuid,
        sequence: u32,
        sql: &str,
        mode: ReplayMode,
    ) -> QueryResult<usize> {
        let fingerprint = FingerprintBuilder::sql(sql);

        match mode {
            ReplayMode::Replay => {
                let stored = self
                    .store
                    .find_match(record_id, CallType::Postgres, &fingerprint, sequence)
                    .await
                    .ok()
                    .flatten();

                let rows_affected = stored
                    .as_ref()
                    .and_then(|i| i.response["rows_affected"].as_u64())
                    .unwrap_or(0) as usize;

                Ok(rows_affected)
            }
            _ => {
                // In record mode, this should not be called directly
                // Use the actual Diesel execution and call record_insert_result
                Ok(0)
            }
        }
    }

    /// Record the result of an INSERT operation after it executes.
    pub async fn record_insert_result(
        &self,
        record_id: uuid::Uuid,
        sequence: u32,
        sql: &str,
        rows_affected: usize,
        duration_ms: u64,
        build_hash: &str,
        service_name: &str,
        tag: &str,
    ) {
        let fingerprint = FingerprintBuilder::sql(sql);
        let interaction = Interaction {
            id: Uuid::new_v4(),
            record_id,
            parent_id: None,
            sequence,
            call_type: CallType::Postgres,
            fingerprint,
            request: json!({"sql": sql}),
            response: json!({"rows_affected": rows_affected}),
            duration_ms,
            status: CallStatus::Completed,
            error: None,
            recorded_at: chrono::Utc::now(),
            build_hash: build_hash.to_string(),
            service_name: service_name.to_string(),
            tag: tag.to_string(),
        };
        let _ = self.store.write(&interaction).await;
    }

    /// Record or replay a SELECT query that returns typed rows.
    pub async fn load<T>(
        &self,
        record_id: uuid::Uuid,
        sequence: u32,
        sql: &str,
        mode: ReplayMode,
    ) -> QueryResult<Vec<T>>
    where
        T: serde::Serialize + serde::de::DeserializeOwned + Send + 'static,
    {
        let fingerprint = FingerprintBuilder::sql(sql);

        match mode {
            ReplayMode::Replay => {
                let stored = self
                    .store
                    .find_match(record_id, CallType::Postgres, &fingerprint, sequence)
                    .await
                    .ok()
                    .flatten();

                let stored = match stored {
                    Some(s) => s,
                    None => {
                        self.store
                            .find_nearest(record_id, &fingerprint, sequence)
                            .await
                            .ok()
                            .flatten()
                            .ok_or(diesel::result::Error::NotFound)?
                    }
                };

                let rows_val = stored.response["rows"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();

                let rows: Vec<T> = rows_val
                    .into_iter()
                    .map(|v| serde_json::from_value(v).map_err(|e| {
                        diesel::result::Error::DeserializationError(
                            Box::new(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                format!("ditto replay decode error: {e}"),
                            )) as Box<dyn std::error::Error + Send + Sync>
                        )
                    }))
                    .collect::<Result<_, _>>()?;

                Ok(rows)
            }
            _ => {
                // In record mode, return empty - actual execution happens elsewhere
                Ok(vec![])
            }
        }
    }

    /// Record the result of a SELECT query after it executes.
    pub async fn record_select_result<T>(
        &self,
        record_id: uuid::Uuid,
        sequence: u32,
        sql: &str,
        rows: &[T],
        duration_ms: u64,
        build_hash: &str,
        service_name: &str,
        tag: &str,
    ) where
        T: serde::Serialize,
    {
        let fingerprint = FingerprintBuilder::sql(sql);
        let rows_json = serde_json::to_value(rows).unwrap_or(Value::Array(vec![]));
        let interaction = Interaction {
            id: Uuid::new_v4(),
            record_id,
            parent_id: None,
            sequence,
            call_type: CallType::Postgres,
            fingerprint,
            request: json!({"sql": sql}),
            response: json!({"rows": rows_json}),
            duration_ms,
            status: CallStatus::Completed,
            error: None,
            recorded_at: chrono::Utc::now(),
            build_hash: build_hash.to_string(),
            service_name: service_name.to_string(),
            tag: tag.to_string(),
        };
        let _ = self.store.write(&interaction).await;
    }

    /// Get the SQL string from a Diesel query for fingerprinting.
    pub fn query_sql<Q>(query: &Q) -> String
    where
        Q: QueryFragment<Pg>,
    {
        use diesel::query_builder::QueryBuilder;
        let mut builder = <Pg as diesel::backend::Backend>::QueryBuilder::new();
        match query.to_sql(&mut builder, &Pg {}) {
            Ok(()) => builder.finish(),
            Err(_) => "unknown".to_string(),
        }
    }
}

/// Convenience macro to build a `DieselReplayExecutor` from a store reference.
#[macro_export]
macro_rules! diesel_exec {
    ($store:expr) => {
        $crate::diesel::DieselReplayExecutor::new(($store).clone())
    };
}
