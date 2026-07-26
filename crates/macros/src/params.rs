//! Parsing and compile-time checking of a query's parameters.
//!
//! `query!("… $min", min = 18)` supplies `$min` at the call site. The analyzer
//! has already inferred a kind for every parameter the query reads, so three
//! things are checkable before the program runs, and all three are:
//!
//! 1. **Every required parameter is supplied.** A parameter is required unless
//!    a `DEFINE PARAM` default covers it.
//! 2. **Nothing extra is supplied.** A `name = expr` for a `$name` the query
//!    never reads is a typo, and silently binding it would hide the typo.
//! 3. **The value's type matches the inferred kind.** The expansion pins the
//!    expression through `Into<T>`, where `T` is the Rust type that kind
//!    decodes into — so `min = "18"` against `$min: int` is a type error at the
//!    call site.
//!
//! The pin is `Into<T>` rather than a bare `T` so that the obvious literals
//! work: `name = "ada"` against `$name: string` coerces `&str` to `String`,
//! and `min = 18` against `$min: int` is already `i64`. What it will not do is
//! bridge unrelated types, which is the point.
//!
//! When the analyzer could not infer a kind for a parameter, the expansion
//! falls back to requiring only `SurrealValue` — the honest position is that an
//! uninferred parameter is unchecked, not that it is any particular type.

use std::collections::BTreeSet;

use proc_macro2::TokenStream;
use quote::quote;
use surrealguard_workspace::AnalysisOutput;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Expr, Ident, LitStr, Token};

use crate::generate::{rust_type, Scope};

/// One `name = expr` argument.
pub struct Param {
    pub name: Ident,
    pub value: Expr,
}

impl Parse for Param {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let name = input.parse()?;
        input.parse::<Token![=]>()?;
        let value = input.parse()?;
        Ok(Self { name, value })
    }
}

/// A `query!` / `query_file!` invocation: a string literal, then parameters.
pub struct MacroInput {
    /// The query text (`query!`) or the path to read it from (`query_file!`).
    pub source: LitStr,
    /// The parameters supplied at the call site.
    pub params: Vec<Param>,
}

impl Parse for MacroInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let source = input.parse()?;
        let params = if input.is_empty() {
            Vec::new()
        } else {
            input.parse::<Token![,]>()?;
            Punctuated::<Param, Token![,]>::parse_terminated(input)?
                .into_iter()
                .collect()
        };
        Ok(Self { source, params })
    }
}

/// The `.bind(…)` calls to append to the `Query` expression, after checking the
/// supplied parameters against the inferred ones.
///
/// # Errors
///
/// Returns a `syn::Error` — spanned at the offending argument where there is
/// one — if a parameter is supplied twice, is not read by the query, or is
/// required and missing.
pub fn bind_calls(input: &MacroInput, output: &AnalysisOutput) -> syn::Result<Vec<TokenStream>> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for param in &input.params {
        if !seen.insert(param.name.to_string()) {
            return Err(syn::Error::new(
                param.name.span(),
                format!("`{}` is supplied more than once", param.name),
            ));
        }
    }

    // 2. Nothing extra. Reported first, and per-argument, because an unknown
    //    name is nearly always a typo and its span points straight at it.
    for param in &input.params {
        let name = param.name.to_string();
        if !output.inferred_params.iter().any(|p| p.name == name) {
            let known: Vec<String> = output
                .inferred_params
                .iter()
                .map(|p| format!("`{}`", p.name))
                .collect();
            let hint = if known.is_empty() {
                "this query reads no parameters".to_owned()
            } else {
                format!("this query reads {}", known.join(", "))
            };
            return Err(syn::Error::new(
                param.name.span(),
                format!("this query has no parameter `${name}` — {hint}"),
            ));
        }
    }

    // 1. Everything required is present.
    let missing: Vec<String> = output
        .inferred_params
        .iter()
        .filter(|p| p.required && !seen.contains(&p.name))
        .map(|p| format!("`{} = …`", p.name))
        .collect();
    if !missing.is_empty() {
        return Err(syn::Error::new(
            input.source.span(),
            format!(
                "this query needs {} parameter{}: {}",
                missing.len(),
                if missing.len() == 1 { "" } else { "s" },
                missing.join(", ")
            ),
        ));
    }

    // 3. Each value's type matches the inferred kind.
    Ok(input
        .params
        .iter()
        .map(|param| {
            let name = param.name.to_string();
            let value = &param.value;
            let inferred = output
                .inferred_params
                .iter()
                .find(|p| p.name == name)
                .and_then(|p| p.kind.as_ref());

            match inferred {
                Some(kind) => {
                    // A parameter's type is rendered into a throwaway scope: an
                    // object kind would want a generated struct, but a struct
                    // the caller cannot name is useless as an *input* type. So
                    // any kind that needs one falls back to `Value`, which every
                    // object satisfies, and the check lands on the scalar kinds
                    // where it does real work.
                    let mut scope = Scope::default();
                    let ty = rust_type(kind, &mut scope);
                    if scope.defs.is_empty() {
                        quote! {
                            .bind(#name, ::surrealguard_rs::_rt::checked::<#ty, _>(#value))
                        }
                    } else {
                        quote! { .bind(#name, #value) }
                    }
                }
                // Uninferred: accept any `SurrealValue`, and say so by not
                // pretending to check it.
                None => quote! { .bind(#name, #value) },
            }
        })
        .collect())
}
