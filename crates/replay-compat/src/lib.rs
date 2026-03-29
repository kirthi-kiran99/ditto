//! Drop-in re-exports with built-in replay instrumentation.
//!
//! # Quick start
//!
//! 1. Call [`install`] once in `main()` before serving any requests.
//! 2. Replace your client imports with `replay_compat::` equivalents.
//! 3. That's it — all outbound calls are automatically recorded or replayed
//!    based on the `REPLAY_MODE` environment variable.
//!
//! ```rust,ignore
//! // In main():
//! replay_compat::install(
//!     Arc::new(PostgresStore::new(&db_url).await?),
//!     ReplayMode::from_env(),
//! );
//!
//! // In your handlers — change the import, nothing else:
//! // Before: use reqwest::Client;
//! // After:
//! use replay_compat::http::Client;
//!
//! let client = Client::new();
//! let resp = client.get("https://api.example.com/v1/data").send().await?;
//! ```
//!
//! # Testing
//!
//! In tests, avoid `install()` (it panics on a second call per process).
//! Use the explicit-store constructors instead:
//!
//! ```rust,ignore
//! use replay_compat::http::Client;
//! use replay_core::{MockContext, ReplayMode, MOCK_CTX};
//! use replay_store::InMemoryStore;
//!
//! let store = Arc::new(InMemoryStore::new());
//! let client = Client::with_store(store.clone());
//!
//! let ctx = MockContext::new(ReplayMode::Record);
//! MOCK_CTX.scope(ctx, async {
//!     client.get("https://...").send().await.unwrap();
//! }).await;
//! ```

use std::sync::{Arc, OnceLock, RwLock};

use replay_core::{InteractionStore, ReplayMode};

pub mod http;
pub mod redis;
pub mod sql;
pub mod tokio;

// ── global store / mode ───────────────────────────────────────────────────────

static GLOBAL: OnceLock<RwLock<Option<(Arc<dyn InteractionStore>, ReplayMode)>>> = OnceLock::new();

fn global_cell() -> &'static RwLock<Option<(Arc<dyn InteractionStore>, ReplayMode)>> {
    GLOBAL.get_or_init(|| RwLock::new(None))
}

/// One-time setup — call this once in `main()` before handling any requests.
///
/// After calling `install`, [`http::Client::new()`], [`redis::open`], and
/// [`sql::connect`] all use the provided store automatically.
///
/// # Panics
/// Does not panic — subsequent calls simply overwrite the previous value.
/// This enables integration tests to call `install` before each test scenario.
pub fn install(store: Arc<dyn InteractionStore>, mode: ReplayMode) {
    let mut guard = global_cell().write().expect("install: lock poisoned");
    *guard = Some((store, mode));
}

/// The globally installed store.  Panics if [`install`] has not been called.
pub(crate) fn global_store() -> Arc<dyn InteractionStore> {
    global_cell()
        .read()
        .expect("global_store: lock poisoned")
        .as_ref()
        .map(|(s, _)| s.clone())
        .expect("replay_compat::install() has not been called — call it once in main()")
}

/// The globally installed mode.  Panics if [`install`] has not been called.
pub(crate) fn global_mode() -> ReplayMode {
    global_cell()
        .read()
        .expect("global_mode: lock poisoned")
        .as_ref()
        .map(|(_, m)| m.clone())
        .expect("replay_compat::install() has not been called — call it once in main()")
}
