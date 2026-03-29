//! Context-propagating `tokio` facade.
//!
//! Re-exports all of `tokio` unchanged except `task::spawn`, which captures
//! the ambient [`replay_core::MockContext`] before spawning and re-injects it
//! inside the new task.
//!
//! # Usage
//!
//! Replace `use tokio::task::spawn` with `use replay_compat::tokio::task::spawn`.
//! All other `tokio::` symbols can be imported from `tokio` directly.
//!
//! ```rust,ignore
//! // Before
//! use tokio::task::spawn;
//!
//! // After — context propagates automatically into the spawned task
//! use replay_compat::tokio::task::spawn;
//!
//! let handle = spawn(async {
//!     // The MockContext from the parent task is available here.
//!     my_intercepted_client.get("https://...").send().await
//! });
//! ```

// ── task module ───────────────────────────────────────────────────────────────

pub mod task {
    use std::future::Future;

    /// Context-propagating replacement for `tokio::spawn`.
    ///
    /// Captures the current [`replay_core::MockContext`] before spawning the
    /// future and re-injects it into the spawned task so that all replay
    /// interceptors inside see the same record context.
    ///
    /// The signature is identical to `tokio::spawn` — swap the import and
    /// nothing else needs to change.
    pub fn spawn<F>(future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        // `spawn_with_ctx` captures MockContext if present; otherwise falls
        // back to a plain `tokio::spawn`.
        replay_core::spawn_with_ctx(future)
    }

    // Re-export the rest of tokio::task unchanged.
    pub use tokio::task::{spawn_blocking, yield_now, JoinHandle, JoinError};
}

// ── re-export the rest of tokio unchanged ─────────────────────────────────────

pub use tokio::{io, net, signal, sync, time};
