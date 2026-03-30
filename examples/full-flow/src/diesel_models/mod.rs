//! Diesel models and schema for the full-flow example.
//!
//! This module demonstrates how to use ditto's Diesel integration
//! alongside the existing sqlx-based code.

pub mod schema;
pub mod models;

pub use models::*;
