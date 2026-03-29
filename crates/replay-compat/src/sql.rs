//! Drop-in PostgreSQL executor with replay instrumentation.
//!
//! Wraps [`ReplayExecutor`] with an API that mirrors `sqlx::PgPool` for the
//! most common query patterns: `execute`, `fetch_all_as`, `fetch_one_as`, and
//! `fetch_optional_as`.
//!
//! # Usage
//!
//! ```rust,ignore
//! // Before
//! let pool = sqlx::PgPool::connect(&db_url).await?;
//! let rows = sqlx::query_as::<_, Payment>("SELECT * FROM payments WHERE id = $1")
//!     .bind(id)
//!     .fetch_all(&pool)
//!     .await?;
//!
//! // After — change the import and pool type, nothing else
//! let pool = replay_compat::sql::connect(&db_url).await?;
//! let rows: Vec<Payment> = pool
//!     .fetch_all_as("SELECT * FROM payments WHERE id = $1", |q| q.bind(id))
//!     .await?;
//! ```

use std::sync::Arc;

use replay_core::InteractionStore;
use replay_interceptors::ReplayExecutor;
use sqlx::{
    postgres::{PgQueryResult, PgRow},
    FromRow, PgPool,
};

// ── connect ───────────────────────────────────────────────────────────────────

/// Connect to Postgres and return an instrumented pool using the global store.
///
/// Requires [`crate::install`] to have been called first.
pub async fn connect(url: &str) -> Result<Pool, sqlx::Error> {
    connect_with_store(url, crate::global_store()).await
}

/// Connect to Postgres and return an instrumented pool using an explicit store.
///
/// Preferred in tests to avoid touching global state.
pub async fn connect_with_store(
    url:   &str,
    store: Arc<dyn InteractionStore>,
) -> Result<Pool, sqlx::Error> {
    let pg = PgPool::connect(url).await?;
    Ok(Pool(ReplayExecutor::new(pg, store)))
}

// ── pool wrapper ──────────────────────────────────────────────────────────────

/// An instrumented PostgreSQL connection pool.
///
/// Delegates to [`ReplayExecutor`] for all query methods.  In record mode,
/// queries are executed against the real database and stored.  In replay mode,
/// stored results are returned without touching the database.
#[derive(Clone)]
pub struct Pool(ReplayExecutor);

impl Pool {
    /// Execute a non-returning statement (INSERT / UPDATE / DELETE).
    pub async fn execute<F>(&self, sql: &str, binder: F) -> Result<PgQueryResult, sqlx::Error>
    where
        F: FnOnce(
            sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments>,
        ) -> sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments>,
    {
        self.0.execute(sql, binder).await
    }

    /// Execute a SELECT and return typed rows.
    ///
    /// `T` must implement both `sqlx::FromRow` and `serde::{Serialize, Deserialize}`
    /// so rows can be stored as JSON and re-hydrated during replay without a
    /// live database.
    pub async fn fetch_all_as<T, F>(&self, sql: &str, binder: F) -> Result<Vec<T>, sqlx::Error>
    where
        T: for<'r> FromRow<'r, PgRow>
            + serde::Serialize
            + serde::de::DeserializeOwned
            + Send
            + Unpin,
        F: FnOnce(
            sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments>,
        ) -> sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments>,
    {
        self.0.fetch_all_as::<T>(sql, binder).await
    }

    /// Execute a SELECT and return a single required row.
    ///
    /// Returns [`sqlx::Error::RowNotFound`] if no row exists.
    pub async fn fetch_one_as<T, F>(&self, sql: &str, binder: F) -> Result<T, sqlx::Error>
    where
        T: for<'r> FromRow<'r, PgRow>
            + serde::Serialize
            + serde::de::DeserializeOwned
            + Send
            + Unpin,
        F: FnOnce(
            sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments>,
        ) -> sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments>,
    {
        self.0.fetch_one_as::<T>(sql, binder).await
    }

    /// Execute a SELECT and return an optional row.
    pub async fn fetch_optional_as<T, F>(
        &self,
        sql:    &str,
        binder: F,
    ) -> Result<Option<T>, sqlx::Error>
    where
        T: for<'r> FromRow<'r, PgRow>
            + serde::Serialize
            + serde::de::DeserializeOwned
            + Send
            + Unpin,
        F: FnOnce(
            sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments>,
        ) -> sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments>,
    {
        self.0.fetch_optional_as::<T>(sql, binder).await
    }

    /// Access the underlying [`ReplayExecutor`] for advanced use cases.
    pub fn executor(&self) -> &ReplayExecutor {
        &self.0
    }
}
