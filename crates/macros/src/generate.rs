//! Renders an inferred response `Kind` into Rust result types.
//!
//! Object shapes become `struct`s defined inside the macro expansion's own
//! block scope — so the type escapes as a value but its name never does, and
//! the caller gets nested field access without ever writing a type name.
//! Struct idents are a hidden counter (`__Sg0`, `__Sg1`, …); because they are
//! block-local, the names carry no meaning and cannot collide with user code.
//!
//! This is the *nameless* default path. The Surrealix-style semantic naming
//! (`User` / `UserAddress` from field lineage) is reserved for a future named
//! form, needed only when a type must be written in a signature.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use surrealdb_types::{Kind, KindLiteral};
use syn::Ident;

/// Accumulates generated struct definitions and hands out fresh idents.
#[derive(Default)]
pub struct Scope {
    pub defs: Vec<TokenStream>,
    next: usize,
}

impl Scope {
    fn fresh(&mut self) -> Ident {
        let ident = format_ident!("__Sg{}", self.next);
        self.next += 1;
        ident
    }
}

/// Renders `kind` as a Rust type, pushing any needed struct definitions into
/// `scope`. Types resolve through the `surrealguard-rs` runtime crate, so a
/// crate using the macro only needs that one dependency in scope.
pub fn rust_type(kind: &Kind, scope: &mut Scope) -> TokenStream {
    match kind {
        Kind::Bool => quote!(bool),
        Kind::Int => quote!(i64),
        Kind::Float | Kind::Number | Kind::Decimal => quote!(f64),
        Kind::String | Kind::Regex => quote!(String),
        Kind::Uuid => quote!(::surrealguard_rs::_rt::uuid::Uuid),
        Kind::Datetime => {
            quote!(::surrealguard_rs::_rt::chrono::DateTime<::surrealguard_rs::_rt::chrono::Utc>)
        }
        // Surreal serializes durations as strings (`1h30m`), so decode as one.
        Kind::Duration => quote!(String),
        Kind::Bytes => quote!(Vec<u8>),
        Kind::None | Kind::Null => quote!(()),
        Kind::Array(element, _) | Kind::Set(element, _) => {
            let inner = rust_type(element, scope);
            quote!(Vec<#inner>)
        }
        Kind::Either(variants) => either_type(variants, scope),
        Kind::Literal(literal) => literal_type(literal, scope),
        // A record link carries its target table; the id decodes as a string.
        Kind::Record(_) => quote!(String),
        // Open objects, geometry, ranges, files, functions, and `any` decode
        // into a dynamic JSON value.
        _ => quote!(::surrealguard_rs::_rt::serde_json::Value),
    }
}

fn literal_type(literal: &KindLiteral, scope: &mut Scope) -> TokenStream {
    match literal {
        KindLiteral::String(_) => quote!(String),
        KindLiteral::Integer(_) => quote!(i64),
        KindLiteral::Float(_) | KindLiteral::Decimal(_) => quote!(f64),
        KindLiteral::Bool(_) => quote!(bool),
        KindLiteral::Duration(_) => quote!(String),
        KindLiteral::Array(kinds) => {
            let items: Vec<TokenStream> = kinds.iter().map(|k| rust_type(k, scope)).collect();
            quote!((#(#items,)*))
        }
        KindLiteral::Object(fields) => object_struct(fields, scope),
    }
}

/// A closed object → a fresh block-local struct; returns its ident.
fn object_struct(
    fields: &std::collections::BTreeMap<String, Kind>,
    scope: &mut Scope,
) -> TokenStream {
    // Render field types first (may push nested structs), then this struct.
    let rendered: Vec<(Ident, TokenStream)> = fields
        .iter()
        .map(|(name, kind)| {
            let (kind, optional) = strip_none(kind);
            let ty = rust_type(&kind, scope);
            let ty = if optional { quote!(Option<#ty>) } else { ty };
            (field_ident(name), ty)
        })
        .collect();

    let ident = scope.fresh();
    let field_defs = rendered.iter().map(|(name, ty)| quote!(pub #name: #ty));
    scope.defs.push(quote! {
        #[derive(Debug, ::surrealguard_rs::_rt::serde::Deserialize)]
        #[serde(crate = "::surrealguard_rs::_rt::serde")]
        struct #ident { #(#field_defs,)* }
    });
    quote!(#ident)
}

/// `Either[None, T]` → `Option<T>`; other unions collapse to unit for now
/// (a generated enum is future work). Bare `Option` handling for fields lives
/// in [`strip_none`]; this covers `Either` appearing as a standalone type.
fn either_type(variants: &[Kind], scope: &mut Scope) -> TokenStream {
    let (kind, optional) = strip_none(&Kind::Either(variants.to_vec()));
    if optional {
        let inner = rust_type(&kind, scope);
        quote!(Option<#inner>)
    } else {
        quote!(())
    }
}

/// Splits `Either[None, ...]` into the present kind and an optional flag.
fn strip_none(kind: &Kind) -> (Kind, bool) {
    let Kind::Either(variants) = kind else {
        return (kind.clone(), false);
    };
    let present: Vec<Kind> = variants
        .iter()
        .filter(|variant| !matches!(variant, Kind::None | Kind::Null))
        .cloned()
        .collect();
    if present.len() == variants.len() {
        return (kind.clone(), false);
    }
    let kind = match present.as_slice() {
        [] => Kind::None,
        [only] => only.clone(),
        _ => Kind::Either(present),
    };
    (kind, true)
}

/// A SurrealQL field name as a Rust field ident, raw-escaping keywords.
fn field_ident(name: &str) -> Ident {
    match syn::parse_str::<Ident>(name) {
        Ok(ident) => ident,
        Err(_) => Ident::new_raw(name, proc_macro2::Span::call_site()),
    }
}
