//! Drop-in Redis client with replay instrumentation.
//!
//! Uses the same API as `redis-rs` for GET / SET / DEL / EXISTS / INCR.
//! In record mode, commands are executed against the real server and stored.
//! In replay mode, stored values are returned without touching Redis.
//!
//! ```rust,ignore
//! // Open a connection backed by the global store
//! let mut conn = replay_compat::redis::open("redis://localhost")?;
//!
//! // In record mode: hits Redis
//! // In replay mode: returns stored value
//! let val: Option<String> = conn.get("session:abc123").await?;
//! conn.set("idempotency:pay_123", "seen", 300).await?;
//! ```

use std::sync::Arc;

use replay_core::InteractionStore;
use replay_interceptors::{RedisInterceptorError, ReplayRedisClient};

// Re-export the error type so callers don't need to import replay-interceptors.
pub use replay_interceptors::RedisInterceptorError as Error;

// ── open ──────────────────────────────────────────────────────────────────────

/// Open a Redis connection backed by the global store.
///
/// Requires [`crate::install`] to have been called first.
///
/// # Errors
/// Returns [`redis::RedisError`] if the URL is malformed or the client cannot
/// be created.  Connection is lazy — no network call is made here.
pub fn open(url: &str) -> Result<Connection, redis::RedisError> {
    open_with_store(url, crate::global_store())
}

/// Open a Redis connection backed by an explicit store.
///
/// Preferred in tests to avoid touching global state.
pub fn open_with_store(
    url:   &str,
    store: Arc<dyn InteractionStore>,
) -> Result<Connection, redis::RedisError> {
    let client = redis::Client::open(url)?;
    Ok(Connection(ReplayRedisClient::new(client, store)))
}

// ── connection wrapper ────────────────────────────────────────────────────────

/// A Redis connection with replay instrumentation.
///
/// Wraps [`ReplayRedisClient`] to expose a connection-style API that mirrors
/// the `redis::aio::Connection` interface.
#[derive(Clone)]
pub struct Connection(ReplayRedisClient);

impl Connection {
    /// GET key — returns `None` when the key does not exist.
    pub async fn get(&self, key: &str) -> Result<Option<String>, RedisInterceptorError> {
        self.0.get(key).await
    }

    /// SET key with a TTL in seconds.
    pub async fn set(
        &self,
        key:   &str,
        value: &str,
        ttl_s: u64,
    ) -> Result<(), RedisInterceptorError> {
        self.0.set(key, value, ttl_s).await
    }

    /// DEL key — returns `true` if the key existed.
    pub async fn del(&self, key: &str) -> Result<bool, RedisInterceptorError> {
        self.0.del(key).await
    }

    /// EXISTS key.
    pub async fn exists(&self, key: &str) -> Result<bool, RedisInterceptorError> {
        self.0.exists(key).await
    }

    /// INCR key.
    pub async fn incr(&self, key: &str) -> Result<i64, RedisInterceptorError> {
        self.0.incr(key).await
    }
}
