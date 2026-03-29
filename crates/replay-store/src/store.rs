// InteractionStore and StoreError now live in replay-core so the matching
// engine can use them without a circular dependency.
// Re-export from here for backwards compatibility.
pub use replay_core::store::{InteractionStore, RecordingSummary, Result, StoreError};
