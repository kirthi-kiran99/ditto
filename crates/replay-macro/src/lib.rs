mod expand;

use proc_macro::TokenStream;
use syn::{parse_macro_input, ItemFn};

/// Wraps an `async fn` to record its inputs and return value during recording
/// mode, and return the stored value without executing the body during replay
/// mode.
///
/// # Requirements
/// - The function must be `async`.
/// - The return type must implement `serde::Serialize` (for recording) and
///   `serde::de::DeserializeOwned` (for replay).
/// - A global store must be registered via `replay_core::set_global_store()`
///   before any annotated function is called.
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
