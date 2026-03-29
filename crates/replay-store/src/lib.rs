pub mod config;
pub mod memory;
pub mod postgres;
pub mod store;

pub use config::{db_url, redis_url};
pub use memory::InMemoryStore;
pub use postgres::PostgresStore;
pub use store::{InteractionStore, RecordingSummary, StoreError};

use std::sync::Arc;

/// Register an `InMemoryStore` as the global macro store.
/// Useful in tests: call once at the top of the test, pass the same Arc to assertions.
pub fn use_memory_store() -> Arc<InMemoryStore> {
    let store = Arc::new(InMemoryStore::new());
    replay_core::set_global_store(store.clone());
    store
}
