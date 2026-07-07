//! Data statement analyzers.
//!
//! These modules model user data operations and their result shapes.

pub mod create;
pub mod delete;
pub mod insert;
pub mod kill;
pub mod live_select;
pub(crate) mod mutation;
pub mod relate;
pub mod select;
pub mod update;
pub mod upsert;
