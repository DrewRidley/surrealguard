//! Runtime for compile-time-checked SurrealQL.
//!
//! Re-exports the [`query!`] and [`surql!`] macros and provides the types the
//! generated code refers to. A crate using the macros only needs
//! `surrealguard-rs` in scope — everything the expansion references
//! (`serde`, `chrono`, `uuid`, [`Query`], [`RecordLink`]) is reachable through
//! this crate.
//!
//! ```ignore
//! use surrealguard_rs::query;
//!
//! let q = query!("SELECT name, age FROM user");
//! // `q` carries the checked text and its inferred, nameless result type.
//! // Executing it against a live database is the next layer; for now the
//! // result type can be deserialized from any JSON encoding of the rows.
//! ```

use std::marker::PhantomData;

pub use surrealguard_macros::{query, surql};

/// Re-exports the generated code depends on. Hidden: not a stable API surface,
/// only a resolution target for macro output.
#[doc(hidden)]
pub mod _rt {
    pub use chrono;
    pub use serde;
    pub use serde_json;
    pub use uuid;
}

/// A compile-time-checked query paired with its inferred result type `T`.
///
/// `query!("…")` expands to one of these, carrying the (already validated)
/// query text and a `T` that is the nameless struct rendered from the inferred
/// response kind.
pub struct Query<T> {
    /// The validated SurrealQL text.
    pub text: &'static str,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Query<T> {
    #[doc(hidden)]
    pub const fn new(text: &'static str) -> Self {
        Self {
            text,
            _marker: PhantomData,
        }
    }
}

impl<T: serde::de::DeserializeOwned> Query<T> {
    /// Deserializes a JSON encoding of this query's result into `T`.
    ///
    /// Used by tests and, ultimately, the execution layer — running the query
    /// against a live database and decoding the response is the next layer.
    pub fn from_json(&self, json: &str) -> serde_json::Result<T> {
        serde_json::from_str(json)
    }
}

/// A typed reference to a record in table `T`, carried as its id string.
///
/// Scaffolding for the execution layer: record links infer as `RecordLink<T>`
/// so a field that holds a `record<post>` is distinguished from a plain string.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RecordLink<T> {
    /// The record id, e.g. `"post:abc"`.
    pub id: String,
    #[serde(skip)]
    _marker: PhantomData<fn() -> T>,
}
