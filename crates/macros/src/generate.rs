//! Renders an inferred response `Kind` into Rust result types.
//!
//! Object shapes become `struct`s defined inside the macro expansion's own
//! block scope — so the type escapes as a value but its name never does, and
//! the caller gets nested field access without ever writing a type name.
//! Struct idents are a hidden counter (`__Sg0`, `__Sg1`, …); because they are
//! block-local, the names carry no meaning and cannot collide with user code.
//!
//! # Why `SurrealValue` and not `Deserialize`
//!
//! The SDK's `take` is bounded by `surrealdb_types::SurrealValue`, and there is
//! no blanket impl bridging it to serde — a `#[derive(Deserialize)]` struct
//! simply cannot be a `take` target. So generated structs get a hand-written
//! `SurrealValue` impl. Writing it by hand rather than deferring to the derive
//! also buys two things the derive does not: an absent or `NULL` field decodes
//! into an `Option<T>` as `None`, and a decode failure names the field.
//!
//! # Why the kind mapping is exact
//!
//! `SurrealValue::from_value` is strictly variant-matched — it performs no
//! coercion at all. `String::from_value` accepts `Value::String` and nothing
//! else, so a `record<t>` column decoded as `String` is a *runtime* error
//! ("Expected string, got record"), not a stringified id. Likewise `f64`
//! accepts only `Number::Float`, never `Int` or `Decimal`. Every mapping below
//! is therefore the exact type the server's value decodes into; an approximate
//! one would compile and then fail in production.
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
        Kind::Float => quote!(f64),
        // `Number::Decimal` is its own variant; `f64` would reject it.
        Kind::Decimal => quote!(::surrealguard_rs::_rt::Decimal),
        // An unrefined `number` can arrive as any of Int/Float/Decimal, so only
        // the enum itself accepts every value the kind admits.
        Kind::Number => quote!(::surrealguard_rs::_rt::Number),
        Kind::String => quote!(String),
        Kind::Regex => quote!(::surrealguard_rs::_rt::Regex),
        Kind::Uuid => quote!(::surrealguard_rs::_rt::Uuid),
        Kind::Datetime => quote!(::surrealguard_rs::_rt::Datetime),
        // `Value::Duration`, not a string: the server sends a duration value.
        Kind::Duration => quote!(::surrealguard_rs::_rt::Duration),
        // `Value::Bytes`, not an array of numbers.
        Kind::Bytes => quote!(::surrealguard_rs::_rt::Bytes),
        Kind::Geometry(_) => quote!(::surrealguard_rs::_rt::Geometry),
        // `()` decodes `Value::None` only. `Value::Null` is a distinct variant,
        // so `null` has to stay dynamic to decode at all.
        Kind::None => quote!(()),
        Kind::Array(element, _) | Kind::Set(element, _) => {
            let inner = rust_type(element, scope);
            quote!(Vec<#inner>)
        }
        Kind::Either(variants) => either_type(variants, scope),
        Kind::Literal(literal) => literal_type(literal, scope),
        // A record link decodes as a `RecordId`, never a string — the SDK sends
        // `Value::RecordId` and `String::from_value` rejects it outright.
        Kind::Record(_) => quote!(::surrealguard_rs::_rt::RecordId),
        // `null`, open objects, ranges, files, functions, and `any` have no
        // narrower Rust type that accepts every value they admit.
        _ => quote!(::surrealguard_rs::_rt::Value),
    }
}

fn literal_type(literal: &KindLiteral, scope: &mut Scope) -> TokenStream {
    match literal {
        KindLiteral::String(_) => quote!(String),
        KindLiteral::Integer(_) => quote!(i64),
        KindLiteral::Float(_) => quote!(f64),
        KindLiteral::Decimal(_) => quote!(::surrealguard_rs::_rt::Decimal),
        KindLiteral::Bool(_) => quote!(bool),
        KindLiteral::Duration(_) => quote!(::surrealguard_rs::_rt::Duration),
        KindLiteral::Array(kinds) => {
            let items: Vec<TokenStream> = kinds.iter().map(|k| rust_type(k, scope)).collect();
            quote!((#(#items,)*))
        }
        KindLiteral::Object(fields) => object_struct(fields, scope),
    }
}

/// A closed object → a fresh block-local struct plus its `SurrealValue` impl;
/// returns its ident.
fn object_struct(
    fields: &std::collections::BTreeMap<String, Kind>,
    scope: &mut Scope,
) -> TokenStream {
    // Render field types first (may push nested structs), then this struct.
    let rendered: Vec<(String, Ident, TokenStream)> = fields
        .iter()
        .map(|(name, kind)| {
            let (kind, optional) = strip_none(kind);
            let ty = rust_type(&kind, scope);
            let ty = if optional { quote!(Option<#ty>) } else { ty };
            (name.clone(), field_ident(name), ty)
        })
        .collect();

    let ident = scope.fresh();
    let field_defs = rendered.iter().map(|(_, name, ty)| quote!(pub #name: #ty));
    let decodes = rendered
        .iter()
        .map(|(key, name, _)| quote!(#name: ::surrealguard_rs::_rt::field(&mut __object, #key)?));
    let encodes = rendered
        .iter()
        .map(|(key, name, _)| quote!(__object.insert(#key, self.#name);));

    scope.defs.push(quote! {
        #[derive(Debug, Clone, PartialEq)]
        struct #ident { #(#field_defs,)* }

        impl ::surrealguard_rs::_rt::SurrealValue for #ident {
            fn kind_of() -> ::surrealguard_rs::_rt::Kind {
                ::surrealguard_rs::_rt::Kind::Object
            }

            fn into_value(self) -> ::surrealguard_rs::_rt::Value {
                let mut __object = ::surrealguard_rs::_rt::Object::new();
                #(#encodes)*
                ::surrealguard_rs::_rt::Value::Object(__object)
            }

            fn from_value(
                __value: ::surrealguard_rs::_rt::Value,
            ) -> ::core::result::Result<Self, ::surrealguard_rs::_rt::Error> {
                let mut __object = ::surrealguard_rs::_rt::expect_object(__value)?;
                ::core::result::Result::Ok(Self { #(#decodes,)* })
            }
        }
    });
    quote!(#ident)
}

/// `Either[None, T]` → `Option<T>`; a union of real alternatives has no Rust
/// type narrower than `Value` that accepts every variant, so it stays dynamic.
fn either_type(variants: &[Kind], scope: &mut Scope) -> TokenStream {
    let (kind, optional) = strip_none(&Kind::Either(variants.to_vec()));
    if optional {
        // A `None`-only union is just the unit; anything else is `Option<T>`.
        if matches!(kind, Kind::None) {
            return quote!(());
        }
        let inner = rust_type(&kind, scope);
        quote!(Option<#inner>)
    } else {
        quote!(::surrealguard_rs::_rt::Value)
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
