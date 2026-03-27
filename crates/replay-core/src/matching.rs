//! Three-tier matching engine: exact → fuzzy (nearest-sequence) → miss.
//!
//! # Tiers
//!
//! 1. **Exact** — `record_id + call_type + fingerprint + sequence` all match.
//! 2. **Fuzzy** — same fingerprint, nearest sequence within `max_sequence_drift`.
//!    Handles code changes that insert or remove calls between the recording
//!    run and the replay run.
//! 3. **Miss** — no usable stored interaction. The per-call-type [`MissStrategy`]
//!    tells the interceptor what to do next.
//!
//! # Usage
//! ```ignore
//! let engine = MatchingEngine::new(store.clone());
//!
//! match engine.resolve(record_id, CallType::Http, &fingerprint, sequence).await {
//!     MatchOutcome::Exact(i)             => serve_stored_response(i),
//!     MatchOutcome::Fuzzy { interaction, .. } => serve_stored_response(interaction),
//!     MatchOutcome::Miss => match engine.miss_strategy_for(CallType::Http) {
//!         MissStrategy::Error       => return Err(...),
//!         MissStrategy::Passthrough => call_real_service().await,
//!         MissStrategy::Empty       => return Ok(empty_response()),
//!     },
//! }
//! ```

use std::sync::Arc;

use uuid::Uuid;

use crate::{CallType, Interaction, InteractionStore};

// ── MissStrategy ──────────────────────────────────────────────────────────────

/// What an interceptor should do when no stored interaction can be found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissStrategy {
    /// Return an error to the caller. Appropriate for HTTP and Postgres where
    /// a missing recording indicates an unexpected code path.
    Error,

    /// Forward the call to the real upstream service. Useful for functions and
    /// non-deterministic calls that are safe to re-execute.
    Passthrough,

    /// Return a silent empty/default response. Appropriate for write-only Redis
    /// commands (SET, DEL, EXPIRE) where the result is not used by callers.
    Empty,
}

// ── MatchConfig ───────────────────────────────────────────────────────────────

/// Per-call-type miss strategies and drift tolerance.
#[derive(Debug, Clone)]
pub struct MatchConfig {
    /// Miss strategy for HTTP calls (default: [`MissStrategy::Error`]).
    pub http: MissStrategy,
    /// Miss strategy for gRPC calls (default: [`MissStrategy::Error`]).
    pub grpc: MissStrategy,
    /// Miss strategy for Postgres queries (default: [`MissStrategy::Error`]).
    pub postgres: MissStrategy,
    /// Miss strategy for Redis commands (default: [`MissStrategy::Empty`]).
    pub redis: MissStrategy,
    /// Miss strategy for `#[record_io]`-annotated functions
    /// (default: [`MissStrategy::Passthrough`]).
    pub function: MissStrategy,
    /// Maximum sequence number delta allowed for a fuzzy match.
    ///
    /// A drift of `5` means an interaction recorded at sequence `N` will
    /// match a replay request for sequences `N-5` through `N+5`.
    pub max_sequence_drift: u32,
}

impl Default for MatchConfig {
    fn default() -> Self {
        Self {
            http:               MissStrategy::Error,
            grpc:               MissStrategy::Error,
            postgres:           MissStrategy::Error,
            redis:              MissStrategy::Empty,
            function:           MissStrategy::Passthrough,
            max_sequence_drift: 5,
        }
    }
}

// ── MatchOutcome ──────────────────────────────────────────────────────────────

/// Result of a three-tier resolution attempt.
#[derive(Debug)]
pub enum MatchOutcome {
    /// The stored interaction had exactly the requested sequence number.
    Exact(Interaction),

    /// A stored interaction with the same fingerprint was found within
    /// [`MatchConfig::max_sequence_drift`] of the requested sequence.
    Fuzzy {
        interaction: Interaction,
        /// Absolute difference between the stored and requested sequence numbers.
        drift: u32,
    },

    /// No usable stored interaction was found. The caller should consult
    /// [`MatchingEngine::miss_strategy_for`] to decide what to do.
    Miss,
}

// ── MatchingEngine ────────────────────────────────────────────────────────────

/// Stateless resolver that wraps a store with three-tier matching logic.
#[derive(Clone)]
pub struct MatchingEngine {
    store:  Arc<dyn InteractionStore>,
    config: MatchConfig,
}

impl MatchingEngine {
    /// Create with default [`MatchConfig`].
    pub fn new(store: Arc<dyn InteractionStore>) -> Self {
        Self { store, config: MatchConfig::default() }
    }

    /// Create with a custom [`MatchConfig`].
    pub fn with_config(store: Arc<dyn InteractionStore>, config: MatchConfig) -> Self {
        Self { store, config }
    }

    /// The miss strategy to apply for a given call type.
    pub fn miss_strategy_for(&self, call_type: CallType) -> &MissStrategy {
        match call_type {
            CallType::Http     => &self.config.http,
            CallType::Grpc     => &self.config.grpc,
            CallType::Postgres => &self.config.postgres,
            CallType::Redis    => &self.config.redis,
            CallType::Function => &self.config.function,
        }
    }

    /// Resolve a stored interaction via the three-tier chain.
    ///
    /// 1. **Exact**: `find_match(record_id, call_type, fingerprint, sequence)`
    /// 2. **Fuzzy**: `find_nearest(record_id, fingerprint, sequence)` if drift
    ///    ≤ [`MatchConfig::max_sequence_drift`]
    /// 3. **Miss**: returns [`MatchOutcome::Miss`]
    pub async fn resolve(
        &self,
        record_id:   Uuid,
        call_type:   CallType,
        fingerprint: &str,
        sequence:    u32,
    ) -> MatchOutcome {
        // Tier 1 — exact
        if let Ok(Some(interaction)) = self
            .store
            .find_match(record_id, call_type, fingerprint, sequence)
            .await
        {
            return MatchOutcome::Exact(interaction);
        }

        // Tier 2 — fuzzy nearest-sequence
        if let Ok(Some(interaction)) = self
            .store
            .find_nearest(record_id, fingerprint, sequence)
            .await
        {
            let drift = (interaction.sequence as i64 - sequence as i64).unsigned_abs() as u32;
            if drift <= self.config.max_sequence_drift {
                return MatchOutcome::Fuzzy { interaction, drift };
            }
        }

        MatchOutcome::Miss
    }
}
