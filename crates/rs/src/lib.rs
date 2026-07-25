//! Compile-time-checked, typed SurrealQL for Rust.
//!
//! `surrealguard-rs` runs the [SurrealGuard](https://github.com/DrewRidley/surrealguard)
//! analyzer against your schema *during compilation*. A wrong table, unknown
//! field, bad function arity, or kind mismatch becomes a `cargo check` error,
//! and the result type is generated from the inferred response — no build
//! script, no language server, no runtime schema fetch. It is the Rust
//! counterpart to the `@surrealguard/*` TypeScript toolchain.
//!
//! # The macros
//!
//! This crate re-exports both proc-macros from
//! [`surrealguard-macros`](https://crates.io/crates/surrealguard-macros) and
//! provides the runtime types their output refers to, so depending on
//! `surrealguard-rs` alone is enough — everything an expansion needs (`serde`,
//! `chrono`, `uuid`, [`Query`], [`RecordLink`]) is reachable through it.
//!
//! - [`query!`] checks a query **and** returns a typed [`Query<T>`](Query),
//!   where `T` is the inferred result rendered as a *nameless* struct. You get
//!   nested field access without ever writing a type — the same ergonomics as
//!   sqlx's anonymous `query!` record.
//! - [`surql!`] checks a query and expands to its validated text as a
//!   `&'static str` — the lighter form when you only want validation.
//!
//! ```rust,ignore
//! use surrealguard_rs::query;
//!
//! let users = query!("SELECT name, age FROM user");
//! //  users: Query<Vec<{ name: String, age: i64 }>>   ← inferred, nameless
//!
//! let bad = query!("SELECT name, ssn FROM user");
//! //  error: SurrealGuard rejected this query:
//! //    [E1002] unknown field `ssn` on table `user`
//! ```
//!
//! # Schema awareness
//!
//! The macros resolve your schema at compile time from, in order:
//!
//! 1. the `SURREALGUARD_SCHEMA` environment variable — a `.surql` file or a
//!    directory, relative to `CARGO_MANIFEST_DIR` unless absolute;
//! 2. otherwise a convention path under the crate root, tried in turn:
//!    `schema/`, then `migrations/`, then `schema.surql`.
//!
//! A directory contributes every `.surql`/`.surrealql` file **sorted by name**,
//! so zero-padded migrations (`0001_*.surql`, `0002_*.surql`, …) apply in
//! order. Each schema file is tracked with `include_bytes!`, so editing it
//! forces a rebuild of the crate that calls the macro. With no schema
//! configured, queries are still checked for everything that does not depend on
//! one (syntax, function arity, operators, …).
//!
//! # `Kind` → Rust mapping
//!
//! The generated result type derives [`serde::Deserialize`]. SurrealDB's
//! `Kind` maps to Rust as:
//!
//! | SurrealQL kind      | Rust type                       |
//! |---------------------|---------------------------------|
//! | `bool`              | `bool`                          |
//! | `int`               | `i64`                           |
//! | `float` / `decimal` | `f64`                           |
//! | `string`            | `String`                        |
//! | `datetime`          | `chrono::DateTime<Utc>`         |
//! | `uuid`              | `uuid::Uuid`                    |
//! | `option<T>`         | `Option<T>`                     |
//! | `array<T>`          | `Vec<T>`                        |
//! | closed `object`     | a nested, nameless struct       |
//! | `record<t>`         | [`RecordLink<T>`](RecordLink)   |
//! | `any` / open object | `serde_json::Value`             |
//!
//! # Execution
//!
//! This crate is the *type-and-check* layer; running the query is deliberately
//! a separate concern. A [`Query<T>`](Query) carries the validated text and its
//! inferred `T`, and [`Query::from_json`] decodes any JSON encoding of the rows
//! into `T`. Executing a [`Query<T>`](Query) against a live database — via the
//! official `surrealdb` Rust SDK — is the next layer up.
//!
//! ```rust,ignore
//! use surrealguard_rs::query;
//!
//! let q = query!("SELECT id FROM user");
//! // `q.text` is the validated SurrealQL; `q.from_json(..)` decodes rows into
//! // the inferred result type once you have a response in hand.
//! let rows = q.from_json(&response_json)?;
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
