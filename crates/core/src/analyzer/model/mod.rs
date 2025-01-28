// model/mod.rs
//! Core type system and data models for SurrealGuard
//!
//! This module contains the fundamental type representations and models used
//! throughout the query analysis and permission validation system. The type
//! system is designed to accurately model both schema definitions and runtime
//! query behaviors.

mod macros;
mod types;

pub use macros::*;
pub use types::*;
