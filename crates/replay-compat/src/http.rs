//! Drop-in HTTP client backed by `reqwest` with replay instrumentation.
//!
//! Identical API surface to `reqwest::Client`.  Swap the import — nothing else
//! changes in your handler code.
//!
//! ```rust,ignore
//! // Before
//! use reqwest::Client;
//!
//! // After
//! use replay_compat::http::Client;
//!
//! let client = Client::new();  // uses global store from install()
//! let resp = client.get("https://api.example.com/payments").send().await?;
//! ```

use std::sync::Arc;

use replay_core::InteractionStore;
use replay_interceptors::ReplayMiddleware;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};

// Re-export the types callers use directly so they need no extra import.
pub use reqwest::{header, Error, Response, StatusCode};
pub use reqwest_middleware::RequestBuilder;

// ── client ────────────────────────────────────────────────────────────────────

/// Drop-in replacement for `reqwest::Client` with replay instrumentation.
///
/// Every outbound HTTP request is automatically recorded or served from the
/// store depending on the active [`replay_core::ReplayMode`].
///
/// # Constructors
/// - [`Client::new()`]        — uses the global store (requires [`crate::install`])
/// - [`Client::with_store()`] — uses an explicit store (preferred in tests)
pub struct Client(ClientWithMiddleware);

impl Client {
    /// Create a client using the globally installed store.
    ///
    /// Requires [`crate::install`] to have been called first.
    pub fn new() -> Self {
        Self::with_store(crate::global_store())
    }

    /// Create a client backed by the given store.
    ///
    /// Use this constructor in unit/integration tests to avoid touching the
    /// global state.
    pub fn with_store(store: Arc<dyn InteractionStore>) -> Self {
        let inner = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("replay_compat: failed to build reqwest client");

        let client = ClientBuilder::new(inner)
            .with(ReplayMiddleware::new(store))
            .build();

        Self(client)
    }

    // ── request builder delegation ────────────────────────────────────────────

    pub fn get(&self, url: impl reqwest::IntoUrl) -> RequestBuilder {
        self.0.get(url)
    }

    pub fn post(&self, url: impl reqwest::IntoUrl) -> RequestBuilder {
        self.0.post(url)
    }

    pub fn put(&self, url: impl reqwest::IntoUrl) -> RequestBuilder {
        self.0.put(url)
    }

    pub fn patch(&self, url: impl reqwest::IntoUrl) -> RequestBuilder {
        self.0.patch(url)
    }

    pub fn delete(&self, url: impl reqwest::IntoUrl) -> RequestBuilder {
        self.0.delete(url)
    }

    pub fn head(&self, url: impl reqwest::IntoUrl) -> RequestBuilder {
        self.0.head(url)
    }

    pub fn request(
        &self,
        method: reqwest::Method,
        url: impl reqwest::IntoUrl,
    ) -> RequestBuilder {
        self.0.request(method, url)
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}
