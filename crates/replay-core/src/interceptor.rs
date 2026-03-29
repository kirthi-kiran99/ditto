use async_trait::async_trait;
use serde_json::Value;

use crate::types::CallType;

// ── error type ────────────────────────────────────────────────────────────────

/// Infrastructure error produced when the replay runtime fails.
///
/// This is *not* the wrapped client's error — it covers store misses,
/// deserialization failures, and other ditto-internal problems.
///
/// Implementors of [`ReplayInterceptor`] must bridge this into their own `Error`
/// type via `impl From<ReplayError> for MyError`.
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("replay: no stored interaction for fingerprint=`{fingerprint}` seq={sequence}")]
    NotFound {
        fingerprint: String,
        sequence:    u32,
    },
    #[error("replay: store error: {0}")]
    Store(String),
    #[error("replay: failed to deserialize stored response: {0}")]
    Deserialize(String),
}

// ── trait ─────────────────────────────────────────────────────────────────────

/// Extension point for adding record/replay support to any client library.
///
/// Implement this trait to support a client not covered by the built-in
/// interceptors in `replay-interceptors`. Everything else — sequencing, store
/// I/O, mode switching — is handled by [`InterceptorRunner`].
///
/// # Type parameter constraints
/// - `Response` must implement [`serde::de::DeserializeOwned`] so the runner
///   can hydrate stored JSON back into a live value during replay.
/// - `Error` must implement `From<ReplayError>` so the runner can surface
///   infrastructure failures through your error type without an extra channel.
///
/// # Example
/// ```ignore
/// use replay_core::{CallType, ReplayError, ReplayInterceptor};
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize)]
/// pub struct MyResponse { status: u16 }
///
/// #[derive(Debug, thiserror::Error)]
/// pub enum MyError {
///     #[error(transparent)]
///     Replay(#[from] ReplayError),
///     #[error("client error: {0}")]
///     Client(String),
/// }
///
/// pub struct MyInterceptor { client: MyClient }
///
/// impl ReplayInterceptor for MyInterceptor {
///     type Request  = MyRequest;
///     type Response = MyResponse;
///     type Error    = MyError;
///
///     fn call_type(&self) -> CallType { CallType::Http }
///
///     fn fingerprint(&self, req: &MyRequest) -> String {
///         format!("{} {}", req.method, req.path)
///     }
///
///     fn normalize_request(&self, req: &MyRequest) -> Value {
///         serde_json::to_value(req).unwrap()
///     }
///     fn normalize_response(&self, res: &MyResponse) -> Value {
///         serde_json::to_value(res).unwrap()
///     }
///
///     async fn execute(&self, req: MyRequest) -> Result<MyResponse, MyError> {
///         self.client.call(req).await.map_err(|e| MyError::Client(e.to_string()))
///     }
/// }
/// ```
#[async_trait]
pub trait ReplayInterceptor: Send + Sync + 'static {
    /// The input to the intercepted call.
    type Request: Send + Sync;

    /// The output of the intercepted call.
    ///
    /// Must be deserializable from a [`serde_json::Value`] so the runner can
    /// hydrate stored responses during replay.
    type Response: Send + Sync + serde::de::DeserializeOwned;

    /// The error type.
    ///
    /// Must be convertible from [`ReplayError`] so the runner can report
    /// infrastructure failures through the caller's error channel.
    type Error: Send + Sync + std::error::Error + From<ReplayError>;

    // ── required methods ──────────────────────────────────────────────────────

    /// Which system this call belongs to (used for store lookup and matching).
    fn call_type(&self) -> CallType;

    /// Stable, parameter-stripped identifier for this call.
    ///
    /// Must be identical for semantically equivalent calls across different
    /// requests and builds. Use [`crate::FingerprintBuilder`] helpers as a
    /// starting point.
    fn fingerprint(&self, req: &Self::Request) -> String;

    /// Strip volatile fields (auth tokens, timestamps, trace IDs) from the
    /// request before storage. The result must be deterministic.
    fn normalize_request(&self, req: &Self::Request) -> Value;

    /// Strip volatile fields from the response before storage.
    fn normalize_response(&self, res: &Self::Response) -> Value;

    /// Execute the actual call.
    ///
    /// Only invoked in [`crate::ReplayMode::Record`],
    /// [`crate::ReplayMode::Passthrough`], and [`crate::ReplayMode::Off`].
    /// In `Replay` mode the runner returns the stored response without calling
    /// this method.
    async fn execute(&self, req: Self::Request) -> Result<Self::Response, Self::Error>;
}
