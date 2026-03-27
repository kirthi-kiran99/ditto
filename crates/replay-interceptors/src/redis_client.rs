use std::sync::Arc;
use std::time::Instant;

use redis::AsyncCommands;
use replay_core::{
    next_interaction_slot, CallStatus, CallType, FingerprintBuilder, Interaction, ReplayMode,
};
use replay_store::InteractionStore;
use serde_json::{json, Value};
use uuid::Uuid;

/// Wraps a `redis::Client` and intercepts GET / SET / DEL / EXISTS operations.
///
/// In **record** mode: executes the real Redis command, stores key + value.
/// In **replay** mode: returns stored values without touching Redis.
///
/// # Usage
/// ```ignore
/// let redis = ReplayRedisClient::new(redis::Client::open(url)?, store.clone());
///
/// // GET
/// let val: Option<String> = redis.get("session:abc123").await?;
///
/// // SET with TTL
/// redis.set("idempotency:pay_123", "seen", 300).await?;
/// ```
#[derive(Clone)]
pub struct ReplayRedisClient {
    inner: redis::Client,
    store: Arc<dyn InteractionStore>,
}

impl ReplayRedisClient {
    pub fn new(client: redis::Client, store: Arc<dyn InteractionStore>) -> Self {
        Self { inner: client, store }
    }

    // ── GET ───────────────────────────────────────────────────────────────────

    pub async fn get(&self, key: &str) -> Result<Option<String>, RedisInterceptorError> {
        let fingerprint = FingerprintBuilder::redis_key(key);
        let slot = next_interaction_slot(CallType::Redis, fingerprint.clone());

        match slot.as_ref().map(|s| &s.mode) {
            Some(ReplayMode::Replay) => {
                let slot = slot.unwrap();
                let stored = self.lookup(&slot).await;
                Ok(stored.and_then(|i| {
                    match &i.response["value"] {
                        Value::Null   => None,
                        Value::String(s) => Some(s.clone()),
                        other         => Some(other.to_string()),
                    }
                }))
            }

            _ => {
                let mut conn = self.inner.get_async_connection().await?;
                let start    = Instant::now();
                let val: Option<String> = conn.get(key).await?;
                let elapsed  = start.elapsed().as_millis() as u64;

                if let Some(slot) = slot {
                    if matches!(slot.mode, ReplayMode::Record) {
                        self.record(
                            &slot, fingerprint,
                            json!({"op": "GET", "key": key}),
                            json!({"value": val}),
                            elapsed,
                        ).await;
                    }
                }

                Ok(val)
            }
        }
    }

    // ── SET ───────────────────────────────────────────────────────────────────

    pub async fn set(
        &self,
        key:   &str,
        value: &str,
        ttl_s: u64,
    ) -> Result<(), RedisInterceptorError> {
        let fingerprint = FingerprintBuilder::redis_key(key);
        let slot = next_interaction_slot(CallType::Redis, fingerprint.clone());

        match slot.as_ref().map(|s| &s.mode) {
            Some(ReplayMode::Replay) => {
                // In replay mode SET is a silent no-op.
                // We still record that a SET was attempted so the diff engine
                // can flag if the new build stops setting an expected key.
                if let Some(slot) = slot {
                    self.record(
                        &slot, fingerprint,
                        json!({"op": "SET", "key": key, "value": value, "ttl": ttl_s}),
                        json!({"ok": true}),
                        0,
                    ).await;
                }
                Ok(())
            }

            _ => {
                let mut conn = self.inner.get_async_connection().await?;
                let start    = Instant::now();
                let _: () = conn
                    .set_ex(key, value, ttl_s as usize)
                    .await?;
                let elapsed = start.elapsed().as_millis() as u64;

                if let Some(slot) = slot {
                    if matches!(slot.mode, ReplayMode::Record) {
                        self.record(
                            &slot, fingerprint,
                            json!({"op": "SET", "key": key, "value": value, "ttl": ttl_s}),
                            json!({"ok": true}),
                            elapsed,
                        ).await;
                    }
                }

                Ok(())
            }
        }
    }

    // ── DEL ───────────────────────────────────────────────────────────────────

    pub async fn del(&self, key: &str) -> Result<bool, RedisInterceptorError> {
        let fingerprint = FingerprintBuilder::redis_key(key);
        let slot = next_interaction_slot(CallType::Redis, fingerprint.clone());

        match slot.as_ref().map(|s| &s.mode) {
            Some(ReplayMode::Replay) => {
                let slot = slot.unwrap();
                let deleted = self.lookup(&slot).await
                    .and_then(|i| i.response["deleted"].as_bool())
                    .unwrap_or(false);
                Ok(deleted)
            }

            _ => {
                let mut conn   = self.inner.get_async_connection().await?;
                let start      = Instant::now();
                let count: i64 = conn.del(key).await?;
                let elapsed    = start.elapsed().as_millis() as u64;
                let deleted    = count > 0;

                if let Some(slot) = slot {
                    if matches!(slot.mode, ReplayMode::Record) {
                        self.record(
                            &slot, fingerprint,
                            json!({"op": "DEL", "key": key}),
                            json!({"deleted": deleted}),
                            elapsed,
                        ).await;
                    }
                }

                Ok(deleted)
            }
        }
    }

    // ── EXISTS ────────────────────────────────────────────────────────────────

    pub async fn exists(&self, key: &str) -> Result<bool, RedisInterceptorError> {
        let fingerprint = FingerprintBuilder::redis_key(key);
        let slot = next_interaction_slot(CallType::Redis, fingerprint.clone());

        match slot.as_ref().map(|s| &s.mode) {
            Some(ReplayMode::Replay) => {
                let slot = slot.unwrap();
                let exists = self.lookup(&slot).await
                    .and_then(|i| i.response["exists"].as_bool())
                    .unwrap_or(false);
                Ok(exists)
            }

            _ => {
                let mut conn     = self.inner.get_async_connection().await?;
                let start        = Instant::now();
                let count: i64   = conn.exists(key).await?;
                let elapsed      = start.elapsed().as_millis() as u64;
                let exists       = count > 0;

                if let Some(slot) = slot {
                    if matches!(slot.mode, ReplayMode::Record) {
                        self.record(
                            &slot, fingerprint,
                            json!({"op": "EXISTS", "key": key}),
                            json!({"exists": exists}),
                            elapsed,
                        ).await;
                    }
                }

                Ok(exists)
            }
        }
    }

    // ── INCR ──────────────────────────────────────────────────────────────────

    pub async fn incr(&self, key: &str) -> Result<i64, RedisInterceptorError> {
        let fingerprint = FingerprintBuilder::redis_key(key);
        let slot = next_interaction_slot(CallType::Redis, fingerprint.clone());

        match slot.as_ref().map(|s| &s.mode) {
            Some(ReplayMode::Replay) => {
                let slot = slot.unwrap();
                let val = self.lookup(&slot).await
                    .and_then(|i| i.response["value"].as_i64())
                    .unwrap_or(1);
                Ok(val)
            }

            _ => {
                let mut conn  = self.inner.get_async_connection().await?;
                let start     = Instant::now();
                let val: i64  = conn.incr(key, 1).await?;
                let elapsed   = start.elapsed().as_millis() as u64;

                if let Some(slot) = slot {
                    if matches!(slot.mode, ReplayMode::Record) {
                        self.record(
                            &slot, fingerprint,
                            json!({"op": "INCR", "key": key}),
                            json!({"value": val}),
                            elapsed,
                        ).await;
                    }
                }

                Ok(val)
            }
        }
    }

    // ── internal helpers ──────────────────────────────────────────────────────

    async fn lookup(
        &self,
        slot: &replay_core::InteractionSlot,
    ) -> Option<Interaction> {
        self.store
            .find_match(slot.record_id, CallType::Redis, &slot.fingerprint, slot.sequence)
            .await
            .ok()
            .flatten()
            .or(
                // fuzzy fallback
                self.store
                    .find_nearest(slot.record_id, &slot.fingerprint, slot.sequence)
                    .await
                    .ok()
                    .flatten()
            )
    }

    async fn record(
        &self,
        slot:        &replay_core::InteractionSlot,
        fingerprint: String,
        request:     Value,
        response:    Value,
        duration_ms: u64,
    ) {
        let interaction = Interaction {
            id:           Uuid::new_v4(),
            record_id:    slot.record_id,
            parent_id:    None,
            sequence:     slot.sequence,
            call_type:    CallType::Redis,
            fingerprint,
            request,
            response,
            duration_ms,
            status:       CallStatus::Completed,
            error:        None,
            recorded_at:  chrono::Utc::now(),
            build_hash:   slot.build_hash.clone(),
            service_name: String::new(),
        };
        let _ = self.store.write(&interaction).await;
    }
}

// ── error type ────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum RedisInterceptorError {
    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),
}
