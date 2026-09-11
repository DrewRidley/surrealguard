//! Compile-time-checked, typed SurrealQL for Rust.
//!
//! `surrealql-analyzer-rs` runs the [SurrealQL Analyzer](https://github.com/surrealdb/analyzer)
//! analyzer against your schema *during compilation*. A wrong table, unknown
//! field, bad function arity, or kind mismatch becomes a `cargo check` error,
//! the result type is generated from the inferred response, and the query's
//! parameters are type-checked against the kinds their uses imply — no build
//! script, no language server, no runtime schema fetch.
//!
//! ```rust,ignore
//! use surrealql_analyzer_rs::query;
//!
//! let adults = query!("SELECT name, age FROM user WHERE age >= $min", min = 18)
//!     .fetch_all(&db)
//!     .await?;
//!
//! for row in adults {
//!     println!("{} is {}", row.name, row.age);   // String, i64 — inferred
//! }
//! ```
//!
//! # The macros
//!
//! This crate re-exports the proc-macros from
//! [`surrealql-analyzer-macros`](https://crates.io/crates/surrealql-analyzer-macros) and
//! provides the runtime types their output refers to, so depending on
//! `surrealql-analyzer-rs` alone is enough.
//!
//! - [`query!`] checks a query **and** returns a typed [`Query<T>`](Query),
//!   where `T` is the inferred result rendered as a *nameless* struct. You get
//!   nested field access without ever writing a type — the same ergonomics as
//!   sqlx's anonymous `query!` record.
//! - [`query_file!`] is [`query!`] with the SurrealQL read from a file at
//!   compile time, resolved relative to the crate root (as `sqlx::query_file!`
//!   does).
//! - [`surql!`] checks a query and expands to its validated text as a
//!   `&'static str` — the lighter form when you only want validation.
//!
//! # Executing
//!
//! With the default `runtime` feature, a [`Query<T>`](Query) runs against the
//! official `surrealdb` SDK. The verbs mirror sqlx:
//!
//! | method             | returns          | available when                     |
//! |--------------------|------------------|------------------------------------|
//! | [`fetch`]          | `T`              | always — `T` is the analyzed shape |
//! | [`fetch_all`]      | `Vec<T::Row>`    | the query yields a row set         |
//! | [`fetch_one`]      | `T::Row`         | the query yields a row set         |
//! | [`fetch_optional`] | `Option<T::Row>` | the query yields a row set         |
//! | [`execute`]        | `()`             | always                             |
//!
//! [`fetch`]: Query::fetch
//! [`fetch_all`]: Query::fetch_all
//! [`fetch_one`]: Query::fetch_one
//! [`fetch_optional`]: Query::fetch_optional
//! [`execute`]: Query::execute
//!
//! [`fetch`](Query::fetch) is the honest one: it hands back exactly the type
//! the analyzer inferred, whatever its shape. The row-set verbs are gated on
//! the [`Rows`] trait, which is implemented for the shapes a row set can take —
//! `Vec<R>` (a plain `SELECT`) and `Option<R>` (`SELECT … FROM ONLY …`). A
//! query that returns a scalar therefore *cannot* call `fetch_all`; that is a
//! compile error, not a runtime surprise:
//!
//! ```rust,ignore
//! query!("RETURN 1 + 1").fetch_all(&db).await?;
//! //  error: this query does not return a set of rows
//! //         note: use `.fetch(&db)` to get the value this query actually returns
//! ```
//!
//! # Parameters
//!
//! Parameters are supplied by name and checked against the kind the analyzer
//! inferred for each use site. There is no unchecked bind: the macro rejects a
//! missing parameter, an unknown one, and a value whose Rust type cannot become
//! the inferred kind.
//!
//! ```rust,ignore
//! query!("SELECT name FROM user WHERE age > $min", min = 18)      // ok
//! query!("SELECT name FROM user WHERE age > $min", min = "18")    // compile error
//! query!("SELECT name FROM user WHERE age > $min")                // compile error: missing `min`
//! query!("SELECT name FROM user", limit = 10)                     // compile error: no `$limit`
//! ```
//!
//! # Multiple statements
//!
//! When more than one statement in a query responds, `T` is a tuple of the
//! responding statements' types, in source order. Non-responding statements
//! (a bare `LET`, a `DEFINE`) are skipped — they still occupy a slot in the
//! SDK's response, and this crate accounts for that so the tuple lines up with
//! what you wrote.
//!
//! ```rust,ignore
//! let (users, posts) = query!("SELECT name FROM user; SELECT title FROM post;")
//!     .fetch(&db)
//!     .await?;
//! ```
//!
//! Because a tuple is not a row set, `fetch_all` on a multi-statement query is
//! a compile error — you must use [`fetch`](Query::fetch) and destructure.
//!
//! # Schema awareness
//!
//! The macros resolve your schema at compile time from, in order:
//!
//! 1. the `SURREALQL_ANALYZER_SCHEMA` environment variable — a `.surql` file or a
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
//! Generated types implement [`surrealdb_types::SurrealValue`], which is what
//! the SDK's `take` actually requires — *not* `serde::Deserialize`. The mapping
//! is chosen so every generated type decodes the `Value` the server really
//! sends: the SDK performs no coercion whatsoever, so an approximate mapping is
//! a runtime failure rather than a lossy read.
//!
//! | SurrealQL kind      | Rust type                                |
//! |---------------------|------------------------------------------|
//! | `bool`              | `bool`                                   |
//! | `int`               | `i64`                                    |
//! | `float`             | `f64`                                    |
//! | `decimal`           | [`Decimal`](surrealdb_types::Decimal)    |
//! | `number`            | [`Number`](surrealdb_types::Number)      |
//! | `string`            | `String`                                 |
//! | `datetime`          | [`Datetime`](surrealdb_types::Datetime)  |
//! | `duration`          | [`Duration`](surrealdb_types::Duration)  |
//! | `uuid`              | [`Uuid`](surrealdb_types::Uuid)          |
//! | `bytes`             | [`Bytes`](surrealdb_types::Bytes)        |
//! | `regex`             | [`Regex`](surrealdb_types::Regex)        |
//! | `record<t>`         | [`RecordId`](surrealdb_types::RecordId)  |
//! | `geometry`          | [`Geometry`](surrealdb_types::Geometry)  |
//! | `option<T>`         | `Option<T>`                              |
//! | `array<T>` / `set`  | `Vec<T>`                                 |
//! | closed `object`     | a nested, nameless struct                |
//! | `any` / open object | [`Value`](surrealdb_types::Value)        |
//!
//! # Feature flags
//!
//! - **`runtime`** (default) — pulls in the `surrealdb` SDK and enables the
//!   `fetch*` / `execute` methods. Turn it off (`default-features = false`) to
//!   get checking and typing with no SDK dependency; [`Query::decode`] still
//!   decodes a response obtained some other way.
//!
//! The SDK is depended on with `default-features = false`, so it contributes no
//! engine or protocol of its own — the executing crate picks those through its
//! own `surrealdb` dependency, and Cargo unifies the two.

use std::fmt;
use std::marker::PhantomData;

pub use surrealdb_types;
use surrealdb_types::{SurrealValue, Value};
pub use surrealql_analyzer_macros::{query, query_file, surql};

#[doc(hidden)]
pub mod _rt;

/// The result of running a compile-time-checked query.
pub type Result<T> = std::result::Result<T, Error>;

/// What went wrong running a [`Query`].
///
/// Unlike the SDK's error (and unlike `sqlx::Error`), this carries the text of
/// the query that produced it, so a failure names itself without the caller
/// having to correlate it back to a call site.
/// Boxed so that `Result<T, Error>` — which every method on [`Query`] returns —
/// stays pointer-sized. The SDK's own error is several words wide, and this
/// wraps it.
#[derive(Debug)]
pub struct Error(Box<ErrorInner>);

#[derive(Debug)]
struct ErrorInner {
    query: &'static str,
    kind: ErrorKind,
}

/// The specific failure behind an [`Error`].
#[derive(Debug)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The database rejected or failed to run the query.
    Database(surrealdb_types::Error),
    /// A statement's result could not be decoded into its inferred type.
    ///
    /// Reaching this means the analyzer and the server disagree about a
    /// shape — a schema that has drifted from the one compiled against is the
    /// usual cause.
    Decode {
        /// The zero-based index of the statement whose result failed.
        statement: usize,
        /// The underlying conversion failure.
        source: surrealdb_types::Error,
    },
    /// [`Query::fetch_one`] found no rows.
    RowNotFound,
}

impl Error {
    /// The text of the query that failed.
    #[must_use]
    pub const fn query(&self) -> &'static str {
        self.0.query
    }

    /// The specific failure.
    #[must_use]
    pub const fn kind(&self) -> &ErrorKind {
        &self.0.kind
    }

    fn new(query: &'static str, kind: ErrorKind) -> Self {
        Self(Box::new(ErrorInner { query, kind }))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0.kind {
            ErrorKind::Database(source) => write!(f, "query failed: {source}")?,
            ErrorKind::Decode { statement, source } => write!(
                f,
                "statement {statement} returned a value that does not match its inferred type: {source}",
            )?,
            ErrorKind::RowNotFound => f.write_str("expected at least one row, got none")?,
        }
        write!(f, "\n  in: {}", self.0.query)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.0.kind {
            ErrorKind::Database(source) | ErrorKind::Decode { source, .. } => Some(source),
            ErrorKind::RowNotFound => None,
        }
    }
}

/// A result shape that is a set of rows.
///
/// Implemented for the shapes a SurrealQL row set can take: `Vec<R>` for a
/// plain `SELECT`, and `Option<R>` for `SELECT … FROM ONLY …`. The row-set
/// verbs on [`Query`] are bounded by this, so calling [`Query::fetch_all`] on a
/// query that returns a scalar — or on a multi-statement query, whose result is
/// a tuple — is a compile error.
#[diagnostic::on_unimplemented(
    message = "this query does not return a set of rows",
    label = "not a row set",
    note = "`fetch_all`, `fetch_one` and `fetch_optional` apply to queries whose result is a row set (`SELECT`, `CREATE`, `UPDATE`, `DELETE`)",
    note = "use `.fetch(&db)` to get the value this query actually returns"
)]
pub trait Rows {
    /// One row of the set.
    type Row;

    /// The rows, in order.
    fn into_rows(self) -> Vec<Self::Row>;
}

impl<R> Rows for Vec<R> {
    type Row = R;

    fn into_rows(self) -> Vec<R> {
        self
    }
}

impl<R> Rows for Option<R> {
    type Row = R;

    fn into_rows(self) -> Vec<R> {
        self.into_iter().collect()
    }
}

/// A compile-time-checked query paired with its inferred result type `T`.
///
/// [`query!`] expands to one of these, carrying the already-validated query
/// text, the parameters bound at the call site, and a `T` that is the inferred
/// response rendered as a nameless struct.
pub struct Query<T> {
    text: &'static str,
    statements: usize,
    vars: Vec<(String, Value)>,
    decode: fn(Vec<Value>) -> _rt::DecodeResult<T>,
    _marker: PhantomData<fn() -> T>,
}

impl<T> fmt::Debug for Query<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Query")
            .field("text", &self.text)
            .field("bound", &self.bound())
            .finish_non_exhaustive()
    }
}

impl<T> Query<T> {
    /// Builds a query. Called only by the macro expansion, which is the only
    /// thing that can supply a `decode` matching the analyzed `text`.
    #[doc(hidden)]
    #[must_use]
    pub const fn new(
        text: &'static str,
        statements: usize,
        decode: fn(Vec<Value>) -> _rt::DecodeResult<T>,
    ) -> Self {
        Self {
            text,
            statements,
            vars: Vec::new(),
            decode,
            _marker: PhantomData,
        }
    }

    /// Binds one parameter. Called only by the macro expansion, which pins the
    /// value's type to the kind the analyzer inferred for that parameter.
    #[doc(hidden)]
    #[must_use]
    pub fn bind(mut self, name: &str, value: impl SurrealValue) -> Self {
        self.vars.push((name.to_owned(), value.into_value()));
        self
    }

    /// The validated SurrealQL text.
    #[must_use]
    pub const fn text(&self) -> &'static str {
        self.text
    }

    /// The number of top-level statements, which is also the number of result
    /// slots the server returns.
    #[must_use]
    pub const fn statements(&self) -> usize {
        self.statements
    }

    /// The names of the parameters bound at the call site, in bind order.
    #[must_use]
    pub fn bound(&self) -> Vec<&str> {
        self.vars.iter().map(|(name, _)| name.as_str()).collect()
    }

    /// Decodes one [`Value`] per statement into the inferred result type.
    ///
    /// This is the whole typed surface with the `runtime` feature off, and the
    /// seam the `fetch*` methods go through — a response obtained any other way
    /// can be decoded with it. `values` must hold one entry per statement, in
    /// source order.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Decode`] if a statement's value does not match the
    /// type inferred for it.
    pub fn decode(&self, values: Vec<Value>) -> Result<T> {
        (self.decode)(values).map_err(|error| {
            Error::new(
                self.text,
                ErrorKind::Decode {
                    statement: error.statement,
                    source: error.source,
                },
            )
        })
    }
}

#[cfg(feature = "runtime")]
impl<T> Query<T> {
    /// Runs the query and returns one [`Value`] per statement.
    async fn raw<C: surrealdb::Connection>(
        &self,
        db: &surrealdb::Surreal<C>,
    ) -> Result<Vec<Value>> {
        let mut request = db.query(self.text);
        for (name, value) in &self.vars {
            request = request.bind((name.clone(), value.clone()));
        }
        let mut response = request
            .await
            .map_err(|source| Error::new(self.text, ErrorKind::Database(source)))?;

        let mut values = Vec::with_capacity(self.statements);
        for statement in 0..self.statements {
            values.push(
                response
                    .take::<Value>(statement)
                    .map_err(|source| Error::new(self.text, ErrorKind::Database(source)))?,
            );
        }
        Ok(values)
    }

    /// Runs the query and returns exactly the type the analyzer inferred.
    ///
    /// This is the shape-preserving verb: a `SELECT` gives a `Vec` of rows, a
    /// `RETURN` gives the value, `SELECT … FROM ONLY` gives an `Option`, and a
    /// multi-statement query gives a tuple. When the result is a row set,
    /// [`fetch_all`](Self::fetch_all) and friends read better.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Database`] if the query failed, or
    /// [`ErrorKind::Decode`] if the response does not match the inferred type.
    pub async fn fetch<C: surrealdb::Connection>(self, db: &surrealdb::Surreal<C>) -> Result<T> {
        let values = self.raw(db).await?;
        self.decode(values)
    }

    /// Runs the query and discards its result.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Database`] if the query failed.
    pub async fn execute<C: surrealdb::Connection>(self, db: &surrealdb::Surreal<C>) -> Result<()> {
        self.raw(db).await.map(|_| ())
    }
}

/// The row-set verbs.
///
/// `T: Rows` is a bound on each *method* rather than on the impl block on
/// purpose: an unsatisfied method bound is an E0277, which honours the
/// `#[diagnostic::on_unimplemented]` note on [`Rows`], whereas an unsatisfied
/// impl-block bound is an E0599 that reports only `i64: Rows` and no guidance.
#[cfg(feature = "runtime")]
impl<T> Query<T> {
    /// Runs the query and returns every row.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Database`] if the query failed, or
    /// [`ErrorKind::Decode`] if a row does not match its inferred type.
    pub async fn fetch_all<C: surrealdb::Connection>(
        self,
        db: &surrealdb::Surreal<C>,
    ) -> Result<Vec<T::Row>>
    where
        T: Rows,
    {
        self.fetch(db).await.map(Rows::into_rows)
    }

    /// Runs the query and returns its first row, failing if there are none.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::RowNotFound`] if the query matched nothing, plus
    /// the errors [`fetch_all`](Self::fetch_all) returns.
    pub async fn fetch_one<C: surrealdb::Connection>(
        self,
        db: &surrealdb::Surreal<C>,
    ) -> Result<T::Row>
    where
        T: Rows,
    {
        let text = self.text;
        let mut rows = self.fetch(db).await.map(Rows::into_rows)?;
        if rows.is_empty() {
            return Err(Error::new(text, ErrorKind::RowNotFound));
        }
        Ok(rows.swap_remove(0))
    }

    /// Runs the query and returns its first row, if any.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Database`] if the query failed, or
    /// [`ErrorKind::Decode`] if a row does not match its inferred type.
    pub async fn fetch_optional<C: surrealdb::Connection>(
        self,
        db: &surrealdb::Surreal<C>,
    ) -> Result<Option<T::Row>>
    where
        T: Rows,
    {
        self.fetch(db)
            .await
            .map(|rows| rows.into_rows().into_iter().next())
    }
}
