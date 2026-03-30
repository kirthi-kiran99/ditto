//! Drop-in Diesel PostgreSQL executor with replay instrumentation.
//!
//! This module provides a wrapper for Diesel async queries that integrates
//! with ditto's record/replay system.
//!
//! # Usage
//!
//! ```rust,ignore
//! use replay_compat::diesel::{DieselExecutor, AsyncPgConnection};
//! use diesel::prelude::*;
//!
//! // Create connection
//! let mut conn = AsyncPgConnection::establish(&db_url).await?;
//! let diesel_exec = DieselExecutor::new();
//!
//! // Execute with replay support - use owned values in queries
//! let idem_key = idempotency_key.clone(); // clone to owned
//! let existing: Option<Order> = diesel_exec
//!     .first_optional(&mut conn, orders::table.filter(orders::id.eq(idem_key)))
//!     .await?;
//! ```

use std::sync::Arc;
use std::time::Instant;

use diesel::{
    pg::Pg,
    query_builder::{QueryFragment, QueryId},
};
use replay_core::{next_interaction_slot, CallType, FingerprintBuilder, Interaction, InteractionStore, ReplayMode};
use serde_json::{json, Value};
use uuid::Uuid;

// Re-export chrono from sqlx for consistency
use sqlx::types::chrono;

// Re-export QueryResult from diesel for convenience
pub use diesel::QueryResult;
// Re-export AsyncPgConnection from diesel_async
pub use diesel_async::AsyncPgConnection;

// ── DieselExecutor ───────────────────────────────────────────────────────────

/// An instrumented Diesel PostgreSQL executor wrapper.
///
/// Uses the ambient `MockContext` from replay_core to determine record/replay mode.
///
/// # Important
///
/// When using Diesel queries with this executor, prefer owned values over references
/// in your queries (e.g., use `.eq(my_string)` where `my_string` is `String`, not `&str`).
/// This avoids lifetime issues with the async replay system.
#[derive(Clone)]
pub struct DieselExecutor {
    store: Arc<dyn InteractionStore>,
}

impl DieselExecutor {
    /// Create a new executor wrapper using the global store.
    ///
    /// Requires [`crate::install`] to have been called first.
    pub fn new() -> Self {
        Self {
            store: crate::global_store(),
        }
    }

    /// Create a new executor wrapper using an explicit store.
    ///
    /// Preferred in tests to avoid touching global state.
    pub fn with_store(store: Arc<dyn InteractionStore>) -> Self {
        Self { store }
    }

    /// Execute a SELECT query and return typed rows.
    ///
    /// In record mode: executes the query and stores results.
    /// In replay mode: returns stored results without touching the database.
    pub async fn load<T, Q>(
        &self,
        conn: &mut AsyncPgConnection,
        query: Q,
    ) -> QueryResult<Vec<T>>
    where
        T: serde::Serialize + serde::de::DeserializeOwned + Send,
        for<'a> Q: diesel_async::methods::LoadQuery<'a, AsyncPgConnection, T>
            + QueryFragment<Pg>
            + QueryId
            + Send,
    {
        let sql = Self::query_sql(&query);
        let fingerprint = FingerprintBuilder::sql(&sql);
        let slot = next_interaction_slot(CallType::Postgres, fingerprint.clone());

        match slot.as_ref().map(|s| &s.mode) {
            Some(ReplayMode::Replay) => {
                let slot = slot.unwrap();
                let stored = self
                    .store
                    .find_match(slot.record_id, CallType::Postgres, &slot.fingerprint, slot.sequence)
                    .await
                    .ok()
                    .flatten();

                let stored = match stored {
                    Some(s) => s,
                    None => {
                        self.store
                            .find_nearest(slot.record_id, &slot.fingerprint, slot.sequence)
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
                let start = Instant::now();
                let result: Result<Vec<T>, _> = diesel_async::RunQueryDsl::get_results(query, conn).await;
                let elapsed = start.elapsed().as_millis() as u64;

                if let (Ok(ref rows), Some(slot)) = (&result, slot) {
                    if matches!(slot.mode, ReplayMode::Record) {
                        let rows_json = serde_json::to_value(rows).unwrap_or(Value::Array(vec![]));
                        let interaction = Interaction {
                            id: Uuid::new_v4(),
                            record_id: slot.record_id,
                            parent_id: None,
                            sequence: slot.sequence,
                            call_type: CallType::Postgres,
                            fingerprint,
                            request: json!({"sql": sql}),
                            response: json!({"rows": rows_json}),
                            duration_ms: elapsed,
                            status: replay_core::CallStatus::Completed,
                            error: None,
                            recorded_at: chrono::Utc::now(),
                            build_hash: slot.build_hash.clone(),
                            service_name: slot.service_name.clone(),
                            tag: slot.tag.clone(),
                        };
                        let _ = self.store.write(&interaction).await;
                    }
                }

                result
            }
        }
    }

    /// Execute a SELECT and return a single required row.
    pub async fn first<T, Q>(
        &self,
        conn: &mut AsyncPgConnection,
        query: Q,
    ) -> QueryResult<T>
    where
        T: serde::Serialize + serde::de::DeserializeOwned + Send,
        for<'a> Q: diesel_async::methods::LoadQuery<'a, AsyncPgConnection, T>
            + QueryFragment<Pg>
            + QueryId
            + Send,
    {
        let rows = self.load(conn, query).await?;
        rows.into_iter().next().ok_or(diesel::result::Error::NotFound)
    }

    /// Execute a SELECT and return an optional row.
    pub async fn first_optional<T, Q>(
        &self,
        conn: &mut AsyncPgConnection,
        query: Q,
    ) -> QueryResult<Option<T>>
    where
        T: serde::Serialize + serde::de::DeserializeOwned + Send,
        for<'a> Q: diesel_async::methods::LoadQuery<'a, AsyncPgConnection, T>
            + QueryFragment<Pg>
            + QueryId
            + Send,
    {
        let rows = self.load(conn, query).await?;
        Ok(rows.into_iter().next())
    }

    /// Execute an INSERT/UPDATE/DELETE statement.
    ///
    /// In replay mode, returns a fake success result based on stored data.
    pub async fn execute<Q>(
        &self,
        conn: &mut AsyncPgConnection,
        query: Q,
    ) -> QueryResult<usize>
    where
        Q: diesel_async::methods::ExecuteDsl<AsyncPgConnection> + QueryFragment<Pg> + QueryId + Send,
    {
        let sql = Self::query_sql(&query);
        let fingerprint = FingerprintBuilder::sql(&sql);
        let slot = next_interaction_slot(CallType::Postgres, fingerprint.clone());

        match slot.as_ref().map(|s| &s.mode) {
            Some(ReplayMode::Replay) => {
                let slot = slot.unwrap();
                let stored = self
                    .store
                    .find_match(slot.record_id, CallType::Postgres, &slot.fingerprint, slot.sequence)
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
                let start = Instant::now();
                let result = diesel_async::RunQueryDsl::execute(query, conn).await;
                let elapsed = start.elapsed().as_millis() as u64;

                if let (Ok(rows_affected), Some(slot)) = (&result, slot) {
                    if matches!(slot.mode, ReplayMode::Record) {
                        let interaction = Interaction {
                            id: Uuid::new_v4(),
                            record_id: slot.record_id,
                            parent_id: None,
                            sequence: slot.sequence,
                            call_type: CallType::Postgres,
                            fingerprint,
                            request: json!({"sql": sql}),
                            response: json!({"rows_affected": rows_affected}),
                            duration_ms: elapsed,
                            status: replay_core::CallStatus::Completed,
                            error: None,
                            recorded_at: chrono::Utc::now(),
                            build_hash: slot.build_hash.clone(),
                            service_name: slot.service_name.clone(),
                            tag: slot.tag.clone(),
                        };
                        let _ = self.store.write(&interaction).await;
                    }
                }

                result
            }
        }
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

impl Default for DieselExecutor {
    fn default() -> Self {
        Self::new()
    }
}

// Re-export diesel prelude for convenience
pub use diesel::prelude::*;
