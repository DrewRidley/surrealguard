//! Compile-time checked SurrealQL for Rust.
//!
//! Three macros, all running the SurrealQL Analyzer engine at compile time so an
//! invalid query fails `cargo check` — the compiler is the checker, with no
//! external codegen step or language server required.
//!
//! - [`surql`] checks a query and expands to its text as a `&'static str`.
//! - [`query`] checks a query and expands to a `surrealql_analyzer_rs::Query<T>`,
//!   where `T` is the inferred result rendered as block-local structs. The
//!   result type has no user-facing name — you get nested field access without
//!   ever writing a type.
//! - [`query_file`] is [`query`] with the SurrealQL read from a file at compile
//!   time.
//!
//! [`query`] and [`query_file`] therefore require the `surrealql-analyzer-rs` runtime
//! crate in scope (which re-exports all three).
//!
//! ```ignore
//! use surrealql_analyzer_rs::query;
//! let q = query!("SELECT name, age FROM user WHERE age > $min", min = 18);
//! ```
//!
//! All three resolve the project schema at compile time (via the internal
//! `schema` module), so queries are typed and checked against real tables and
//! fields. Any error-severity finding is turned into a `compile_error!`
//! spanned at the string literal, carrying each finding's code and message:
//!
//! ```text
//! error: SurrealQL Analyzer rejected this query:
//!   [E1002] unknown field `ssn` on table `user`
//! ```
//!
//! # Parameters
//!
//! [`query`] and [`query_file`] take the query's parameters as trailing
//! `name = expr` pairs. The analyzer infers each parameter's kind from its use
//! sites, and the expansion pins the supplied expression to the Rust type that
//! kind decodes into — so a wrong-typed argument is a type error, not a runtime
//! one. A missing required parameter, or one the query never reads, is rejected
//! outright. There is no unchecked bind.

mod generate;
mod params;
mod schema;

use std::path::PathBuf;

use proc_macro::TokenStream;
use quote::quote;
use surrealql_analyzer_diagnostics::Severity;
use surrealql_analyzer_workspace::{analyze_workspace, AnalysisOutput, Workspace};
use syn::{parse_macro_input, LitStr};

use params::MacroInput;

/// The result of checking a query: its analysis plus the schema files it was
/// checked against (emitted as `include_bytes!` so edits trigger a rebuild).
struct Checked {
    output: AnalysisOutput,
    schema_paths: Vec<PathBuf>,
}

/// Checks a `SurrealQL` string literal at compile time; expands to its text.
#[proc_macro]
pub fn surql(input: TokenStream) -> TokenStream {
    let literal = parse_macro_input!(input as LitStr);
    let query = literal.value();

    let checked = match check(&query, literal.span()) {
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

/// Checks a `SurrealQL` string literal at compile time and expands to a
/// `surrealql_analyzer_rs::Query<T>` whose `T` is the inferred, nameless result type.
#[proc_macro]
pub fn query(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as MacroInput);
    let text = input.source.value();
    expand(&text, &input, Vec::new())
}

/// Like [`query`], but reads the `SurrealQL` from a file at compile time.
///
/// The path is resolved relative to the crate root (the directory holding
/// `Cargo.toml`), matching `sqlx::query_file!` rather than `include_str!`.
#[proc_macro]
pub fn query_file(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as MacroInput);
    let relative = input.source.value();

    let Some(root) = std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from) else {
        return error_at(
            input.source.span(),
            "`query_file!` needs CARGO_MANIFEST_DIR to resolve its path, and it is not set",
        );
    };
    let path = root.join(&relative);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            return error_at(
                input.source.span(),
                &format!("could not read `{}`: {error}", path.display()),
            )
        }
    };
    expand(&text, &input, vec![path])
}

/// Checks `text`, then renders the `Query<T>` expression for it.
///
/// `extra_tracking` holds paths beyond the schema that must force a rebuild —
/// for [`query_file`], the query file itself.
fn expand(text: &str, input: &MacroInput, extra_tracking: Vec<PathBuf>) -> TokenStream {
    let span = input.source.span();
    let checked = match check(text, span) {
        Ok(checked) => checked,
        Err(error) => return error,
    };

    let binds = match params::bind_calls(input, &checked.output) {
        Ok(binds) => binds,
        Err(error) => return error.to_compile_error().into(),
    };

    let mut paths = checked.schema_paths;
    paths.extend(extra_tracking);
    let tracking = rebuild_tracking(&paths);

    let mut scope = generate::Scope::default();
    let (ty, body, statements) = response(&checked.output, &mut scope);
    let defs = scope.defs;

    quote! {{
        #tracking
        #(#defs)*
        ::surrealql_analyzer_rs::Query::<#ty>::new(
            #text,
            #statements,
            |mut __values: ::std::vec::Vec<::surrealql_analyzer_rs::_rt::Value>| #body,
        ) #(#binds)*
    }}
    .into()
}

/// The result type, the decode closure's *body*, and the statement count.
///
/// A statement that responds occupies a result slot and contributes its type;
/// one that does not (a bare `LET`, a `DEFINE`) still occupies a slot, which is
/// why the decode body indexes by *statement position* rather than by position
/// among the responders.
///
/// The body is returned whole rather than as an expression to wrap in `Ok(…)`,
/// because the single-statement case must expand to a bare `decode_at(…)` call:
/// wrapping it would produce `Ok(x?)`, and clippy's `needless_question_mark`
/// fires on macro output in the *caller's* crate, where it cannot be silenced.
fn response(
    output: &AnalysisOutput,
    scope: &mut generate::Scope,
) -> (proc_macro2::TokenStream, proc_macro2::TokenStream, usize) {
    let responders: Vec<(usize, &surrealdb_types::Kind)> = output
        .statements
        .iter()
        .enumerate()
        .filter_map(|(index, statement)| statement.response_kind.as_ref().map(|kind| (index, kind)))
        .collect();
    let statements = output.statements.len();

    match responders.as_slice() {
        // Nothing responds: a schema-only or LET-only query.
        [] => (
            quote!(()),
            quote!({ ::core::result::Result::Ok(()) }),
            statements,
        ),
        [(index, kind)] => {
            let ty = generate::rust_type(kind, scope);
            let body = quote! {
                { ::surrealql_analyzer_rs::_rt::decode_at::<#ty>(&mut __values, #index) }
            };
            (ty, body, statements.max(1))
        }
        many => {
            let types: Vec<proc_macro2::TokenStream> = many
                .iter()
                .map(|(_, kind)| generate::rust_type(kind, scope))
                .collect();
            let decodes = many.iter().zip(&types).map(|((index, _), ty)| {
                quote!(::surrealql_analyzer_rs::_rt::decode_at::<#ty>(&mut __values, #index)?)
            });
            let body = quote! {
                { ::core::result::Result::Ok((#(#decodes,)*)) }
            };
            (quote!((#(#types,)*)), body, statements)
        }
    }
}

/// Runs the analyzer against the resolved schema and the query, turning
/// error-severity findings into a `compile_error!` (spanned at the literal).
fn check(query: &str, span: proc_macro2::Span) -> Result<Checked, TokenStream> {
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
        let message = format!(
            "SurrealQL Analyzer rejected this query:\n{}",
            errors.join("\n")
        );
        return Err(syn::Error::new(span, message).to_compile_error().into());
    }

    Ok(Checked {
        output,
        schema_paths: schema_files.into_iter().map(|(path, _)| path).collect(),
    })
}

/// A `compile_error!` at `span`.
fn error_at(span: proc_macro2::Span, message: &str) -> TokenStream {
    syn::Error::new(span, message).to_compile_error().into()
}

/// Emits an `include_bytes!` per tracked file so that editing it forces the
/// calling crate to recompile (proc-macro output is otherwise only rebuilt when
/// the call site changes).
fn rebuild_tracking(paths: &[PathBuf]) -> proc_macro2::TokenStream {
    let includes = paths.iter().map(|path| {
        let path = path.to_string_lossy();
        quote! { const _: &[u8] = include_bytes!(#path); }
    });
    quote! { #(#includes)* }
}
