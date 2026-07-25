//! Compile-time checked SurrealQL for Rust.
//!
//! Two macros, both running the SurrealGuard analyzer at compile time so an
//! invalid query fails `cargo check` — the compiler is the checker, with no
//! external codegen step or language server required.
//!
//! - [`surql`] checks a query and expands to its text as a `&'static str`.
//! - [`query`] checks a query and expands to a `surrealguard_rs::Query<T>`,
//!   where `T` is the inferred result rendered as block-local structs. The
//!   result type has no user-facing name — you get nested field access without
//!   ever writing a type. `query!` therefore requires the `surrealguard-rs`
//!   runtime crate in scope (which re-exports these macros).
//!
//! ```ignore
//! use surrealguard_rs::query;
//! let q = query!("SELECT name, age FROM user");   // Query<Vec<{ name, age }>>
//! ```
//!
//! Both macros resolve the project schema at compile time (via the internal
//! `schema` module), so queries are typed and checked against real tables and
//! fields. Any error-severity finding is turned into a `compile_error!`
//! spanned at the string literal, carrying each finding's code and message:
//!
//! ```text
//! error: SurrealGuard rejected this query:
//!   [E1002] unknown field `ssn` on table `user`
//! ```
//!
//! These macros are re-exported by, and intended to be used through, the
//! [`surrealguard-rs`](https://crates.io/crates/surrealguard-rs) runtime crate,
//! which also provides the `Query<T>` and `RecordLink<T>` types `query!` refers
//! to.

mod generate;
mod schema;

use std::path::PathBuf;

use proc_macro::TokenStream;
use quote::quote;
use surrealguard_diagnostics::Severity;
use surrealguard_workspace::{analyze_workspace, AnalysisOutput, Workspace};
use syn::{parse_macro_input, LitStr};

/// The result of checking a query: its analysis plus the schema files it was
/// checked against (emitted as `include_bytes!` so edits trigger a rebuild).
struct Checked {
    output: AnalysisOutput,
    schema_paths: Vec<PathBuf>,
}

/// Checks a SurrealQL string literal at compile time; expands to its text.
#[proc_macro]
pub fn surql(input: TokenStream) -> TokenStream {
    let literal = parse_macro_input!(input as LitStr);
    let query = literal.value();

    let checked = match check(&query, &literal) {
        Ok(checked) => checked,
        Err(error) => return error,
    };
    let tracking = rebuild_tracking(&checked.schema_paths);
    quote! {{
        #tracking
        #query
    }}
    .into()
}

/// Checks a SurrealQL string literal at compile time and expands to a
/// `surrealguard_rs::Query<T>` whose `T` is the inferred, nameless result type.
#[proc_macro]
pub fn query(input: TokenStream) -> TokenStream {
    let literal = parse_macro_input!(input as LitStr);
    let query = literal.value();

    let checked = match check(&query, &literal) {
        Ok(checked) => checked,
        Err(error) => return error,
    };
    let tracking = rebuild_tracking(&checked.schema_paths);

    // A statement that produces no response (e.g. a bare LET) yields `()`.
    let kind = checked
        .output
        .response_kind
        .unwrap_or(surrealdb_types::Kind::Null);

    let mut scope = generate::Scope::default();
    let ty = generate::rust_type(&kind, &mut scope);
    let defs = scope.defs;
    quote! {{
        #tracking
        #(#defs)*
        ::surrealguard_rs::Query::<#ty>::new(#query)
    }}
    .into()
}

/// Runs the analyzer against the resolved schema and the query, turning
/// error-severity findings into a `compile_error!` (spanned at the literal).
fn check(query: &str, literal: &LitStr) -> Result<Checked, TokenStream> {
    let schema_files = schema::load();

    let mut workspace = Workspace::default();
    for (path, text) in &schema_files {
        workspace.add_file_source(path.clone(), text.clone());
    }
    // The query sorts after every `file://` schema source, so the schema is
    // analyzed first and its tables/fields are in scope for the query.
    let query_id = workspace.add_virtual_source("surql_query".into(), query.into());

    let analysis = analyze_workspace(&workspace);
    let output = analysis.sources.get(&query_id).cloned().unwrap_or_default();

    let errors: Vec<String> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.severity() == Severity::Error)
        .map(|finding| format!("  [{}] {}", finding.code(), finding.message()))
        .collect();

    if !errors.is_empty() {
        // On stable Rust the span covers the whole literal; the message carries
        // each finding's code and text. (Precise sub-literal spans need the
        // unstable `proc_macro_span` API.)
        let message = format!("SurrealGuard rejected this query:\n{}", errors.join("\n"));
        return Err(syn::Error::new(literal.span(), message)
            .to_compile_error()
            .into());
    }

    Ok(Checked {
        output,
        schema_paths: schema_files.into_iter().map(|(path, _)| path).collect(),
    })
}

/// Emits an `include_bytes!` per schema file so that editing the schema forces
/// the query's crate to recompile (proc-macro output is otherwise only rebuilt
/// when the call site changes).
fn rebuild_tracking(paths: &[PathBuf]) -> proc_macro2::TokenStream {
    let includes = paths.iter().map(|path| {
        let path = path.to_string_lossy();
        quote! { const _: &[u8] = include_bytes!(#path); }
    });
    quote! { #(#includes)* }
}
