mod expand;
mod instrument;
mod instrument_spawns;
mod instrument_trait;

use proc_macro::TokenStream;
use syn::{parse_macro_input, ItemFn, ItemImpl, ItemTrait};

/// Wraps an `async fn` to record its inputs and return value during recording
/// mode, and return the stored value without executing the body during replay
/// mode.
///
/// # Requirements
/// - The function must be `async`.
/// - The return type must implement `serde::Serialize` (for recording) and
///   `serde::de::DeserializeOwned` (for replay).
///
/// # Example
/// ```ignore
/// use replay_macro::record_io;
///
/// #[record_io]
/// async fn compute_fee(amount: u64, currency: String) -> u64 {
///     amount * 2 + 100
/// }
/// ```
#[proc_macro_attribute]
pub fn record_io(_args: TokenStream, input: TokenStream) -> TokenStream {
    let func = parse_macro_input!(input as ItemFn);
    match expand::expand_record_io(func) {
        Ok(tokens) => tokens.into(),
        Err(e)     => e.to_compile_error().into(),
    }
}

/// Instruments every `async fn` in an `impl` block with record/replay logic.
///
/// Sync methods pass through unchanged.  Apply this macro once to the whole
/// `impl` block instead of annotating each method individually.
///
/// # Example
/// ```ignore
/// use replay_macro::instrument;
///
/// #[instrument]
/// impl MyService {
///     async fn fetch_data(&self, id: u64) -> Data { ... }
///     fn cache_key(&self, id: u64) -> String { ... } // unchanged
/// }
/// ```
#[proc_macro_attribute]
pub fn instrument(_args: TokenStream, input: TokenStream) -> TokenStream {
    let impl_block = parse_macro_input!(input as ItemImpl);
    match instrument::expand_instrument(impl_block) {
        Ok(tokens) => tokens.into(),
        Err(e)     => e.to_compile_error().into(),
    }
}

/// Rewrites every `tokio::spawn(…)` call inside a function to
/// `replay_core::spawn_with_ctx(…)` so that the ambient `MockContext` is
/// automatically propagated into spawned tasks.
///
/// # Example
/// ```ignore
/// use replay_macro::instrument_spawns;
///
/// #[instrument_spawns]
/// async fn handler(req: Request) -> Response {
///     tokio::spawn(async { background_work().await });
///     // ↑ automatically becomes replay_core::spawn_with_ctx(...)
///     build_response().await
/// }
/// ```
#[proc_macro_attribute]
pub fn instrument_spawns(_args: TokenStream, input: TokenStream) -> TokenStream {
    let func = parse_macro_input!(input as ItemFn);
    instrument_spawns::expand_instrument_spawns(func).into()
}

/// Generates a context-propagating wrapper struct for a trait.
///
/// Annotating a trait with `#[instrument_trait]` re-emits the original trait
/// and additionally generates `ReplayMyTrait<T: MyTrait>`.  Each async method
/// in the wrapper delegates to `self.0.method(…).await` inside the standard
/// record/replay envelope.
///
/// # Example
/// ```ignore
/// use replay_macro::instrument_trait;
///
/// #[instrument_trait]
/// pub trait PaymentGateway {
///     async fn charge(&self, amount: u64) -> ChargeResult;
///     async fn refund(&self, charge_id: &str) -> RefundResult;
/// }
///
/// // Generates: pub struct ReplayPaymentGateway<T: PaymentGateway>(pub T);
/// // impl<T: PaymentGateway> PaymentGateway for ReplayPaymentGateway<T> { ... }
/// ```
#[proc_macro_attribute]
pub fn instrument_trait(_args: TokenStream, input: TokenStream) -> TokenStream {
    let trait_def = parse_macro_input!(input as ItemTrait);
    match instrument_trait::expand_instrument_trait(trait_def) {
        Ok(tokens) => tokens.into(),
        Err(e)     => e.to_compile_error().into(),
    }
}
