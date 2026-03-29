//! Framework adapters for injecting [`replay_core::MockContext`] at request
//! boundaries.
//!
//! | Module | Framework | Feature flag |
//! |--------|-----------|--------------|
//! | [`tower_layer`] | Any Tower-compatible (Axum, hyper, …) | *(always compiled)* |
//! | [`actix`] | Actix-web 4 | `actix-middleware` |
//!
//! # Choosing between Axum helpers and the Tower layer
//!
//! If you are already using Axum, the convenience functions in
//! [`crate::http_server`] (`recording_middleware`, `recording_middleware_with_store`)
//! work fine.  The [`RecordingLayer`] in this module is the portable alternative
//! for non-Axum stacks.

pub mod tower_layer;

#[cfg(feature = "actix-middleware")]
pub mod actix;

pub use tower_layer::{RecordingLayer, RecordingService};
