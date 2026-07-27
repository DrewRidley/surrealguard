//! TypeScript generation from analysis results.
//!
//! Two layers: [`ts_type`] renders one `surrealdb_types::Kind` as a
//! TypeScript type *for a named position* ([`TsContext`]), and
//! [`render_registry`] emits the generated `.d.ts`
//! — a literal-keyed registry mapping each embedded query to its result
//! type, substitution tuple, and named-parameter object, plus the `surql`
//! tag and `SurqlQuery` carrier the host code consumes.
//!
//! Value conventions (documented in the generated header) name the values
//! the SurrealDB SDK **actually decodes**, which are its own value classes:
//! `record` is `RecordId<"table">`, `uuid` is `Uuid`, `duration` is
//! `Duration`, `decimal` is `Decimal`, and `datetime` is a native `Date`
//! because `createClient` sets `codecOptions.useNativeDates`. `NONE` is
//! `undefined`. These are not cosmetic: a `RecordId` param encodes to a
//! record link on the wire (CBOR tag 8) while a plain string encodes to a
//! SurrealQL string, so `WHERE team = $team` only matches with the class.
//!
//! # Optionality has two spellings, and the position picks
//!
//! A SurrealQL `option<string>` is `Either([None, String])`, and TypeScript
//! has two genuinely different ways to say it. As the type of an object
//! member it is `nick?: string` — the *key* may be absent. Anywhere a value
//! stands on its own it is `string | undefined` — there is no key to omit,
//! so the absence has to be a union member. They are not interchangeable:
//! `{ nick?: string }` accepts `{}` and `{ nick: string | undefined }` does
//! not.
//!
//! Before [`TsContext`] the choice was made by *which function happened to
//! be running*: `object_type` was the only caller that stripped the `none`,
//! so the same kind came out `nick?: string` one level down and
//! `undefined | string` at the top of a response. The behaviour was right;
//! it just was not stated. Now the caller names its position and the two
//! spellings are a decision rather than an artifact.

use surrealdb_types::{Kind, KindLiteral};
use surrealguard_workspace::analysis::{ParamInference, ValueDomain};

mod registry;

pub use registry::{render_registry, QueryEntry};

/// Where the rendered text is going to sit in a TypeScript type.
///
/// The axis is not nesting depth — an array element is nested and still a
/// value — but whether the position owns a **key that can be absent**. That
/// is the only question the two spellings of `option<T>` answer differently,
/// and it is the only question this crate's emitter has ever had to ask, so
/// the enum has exactly two variants.
///
/// Deliberately *not* mirrored from `KindContext`
/// (`surrealguard_workspace::render`), the SurrealQL-side audience enum:
///
/// - no `Declared`, because TypeScript has no author-written spelling to
///   mirror. On the SurrealQL side `option<string>` is text the reader can go
///   and find in a `DEFINE FIELD`; here the generated file *is* the
///   declaration, and `option<…>` is not TypeScript.
/// - no `Occurrence`, because codegen renders declared schema kinds. Flow
///   narrowing is a fact about one point in a query, and no point in a query
///   reaches the `.d.ts`.
/// - no `Glance`, because nothing here has a character budget. A `.d.ts` that
///   widened a type to fit a line would be lying to `tsc`.
/// - no `Diagnostic`, because the generated file has no reader to blame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TsContext {
    /// A type standing on its own: a tuple element, an array element, a union
    /// member, a type argument, a statement's result. There is no key here,
    /// so absence is a union member — `option<string>` is `undefined | string`.
    Value,
    /// The type of one member of an object type, where the key itself may be
    /// omitted. An `option<T>` folds its `none` into the key's `?` marker
    /// rather than into the value, giving `nick?: string`.
    ///
    /// The `?` is reported back as [`TsType::optional`] rather than baked into
    /// the text: the marker sits before the colon, so only the caller — which
    /// is writing the key — can place it.
    Property,
}

/// A rendered TypeScript type, plus what the position could not express in
/// the text alone.
///
/// A struct rather than a bare `String` for the same reason the SurrealQL-side
/// `Rendered` is one: [`TsContext::Property`] has an answer that is not part
/// of the type text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TsType {
    /// The type text.
    pub text: String,
    /// Whether the key may be omitted. Always `false` under
    /// [`TsContext::Value`], which has no key to omit.
    pub optional: bool,
}

/// Renders a `Kind` as TypeScript for the position `ctx` names.
pub fn ts_type(kind: &Kind, ctx: TsContext) -> TsType {
    match ctx {
        TsContext::Value => TsType {
            text: value_type(kind),
            optional: false,
        },
        TsContext::Property => {
            let (present, optional) = strip_none(kind);
            TsType {
                text: value_type(&present),
                optional,
            }
        }
    }
}

/// The value spelling — every position that is not an object key. Recursion
/// stays here: an array element, a union member and a tuple item are all
/// values, and only [`object_type`] crosses back into a key.
fn value_type(kind: &Kind) -> String {
    match kind {
        Kind::Any => "unknown".into(),
        Kind::None => "undefined".into(),
        Kind::Null => "null".into(),
        Kind::Bool => "boolean".into(),
        Kind::Int | Kind::Float | Kind::Number => "number".into(),
        // The SDK decodes a SurrealQL `decimal` as its arbitrary-precision
        // `Decimal` class, never a JS number — `number` would silently lose
        // precision and typecheck arithmetic that throws.
        Kind::Decimal => "Decimal".into(),
        Kind::String => "string".into(),
        Kind::Uuid => "Uuid".into(),
        Kind::Duration => "Duration".into(),
        // `Date` is truthful only because `createClient` sets
        // `codecOptions.useNativeDates`; without it the SDK hands back its
        // own `DateTime` class.
        Kind::Datetime => "Date".into(),
        Kind::Bytes => "Uint8Array".into(),
        Kind::Object => "Record<string, unknown>".into(),
        Kind::Array(element, _) | Kind::Set(element, _) => {
            format!("Array<{}>", value_type(element))
        }
        Kind::Record(tables) => match tables.as_slice() {
            [] => "RecordId<string>".into(),
            tables => tables
                .iter()
                .map(|table| format!("RecordId<\"{table}\">"))
                .collect::<Vec<_>>()
                .join(" | "),
        },
        Kind::Either(variants) => {
            let mut rendered: Vec<String> = variants.iter().map(value_type).collect();
            rendered.dedup();
            rendered.join(" | ")
        }
        Kind::Literal(literal) => literal_type(literal),
        Kind::Table(_) | Kind::Range | Kind::Function(..) => "unknown".into(),
        Kind::Geometry(_) => "GeoJSON".into(),
        Kind::File(_) => "unknown".into(),
        Kind::Regex => "string".into(),
    }
}

fn literal_type(literal: &KindLiteral) -> String {
    match literal {
        KindLiteral::String(value) => format!("\"{}\"", value.replace('"', "\\\"")),
        KindLiteral::Integer(value) => value.to_string(),
        KindLiteral::Float(value) => value.to_string(),
        KindLiteral::Decimal(value) => value.to_string(),
        KindLiteral::Bool(value) => value.to_string(),
        KindLiteral::Duration(_) => "Duration".into(),
        // A literal array is a tuple: each item is a value position, so an
        // `option<T>` item stays `T | undefined` — a tuple has no key to omit,
        // and dropping the member would shorten the tuple.
        KindLiteral::Array(kinds) => {
            let items: Vec<String> = kinds.iter().map(value_type).collect();
            format!("[{}]", items.join(", "))
        }
        KindLiteral::Object(fields) => object_type(fields.iter()),
    }
}

/// A closed object type. This is the one place a key exists, so it is the one
/// place that renders in [`TsContext::Property`]: an `option<T>` field becomes
/// an optional key rather than an `undefined` union member.
fn object_type<'a>(fields: impl Iterator<Item = (&'a String, &'a Kind)>) -> String {
    let mut parts = Vec::new();
    for (name, kind) in fields {
        let rendered = ts_type(kind, TsContext::Property);
        let key = if is_identifier(name) {
            name.clone()
        } else {
            format!("\"{}\"", name.replace('"', "\\\""))
        };
        let marker = if rendered.optional { "?" } else { "" };
        parts.push(format!("{key}{marker}: {}", rendered.text));
    }
    if parts.is_empty() {
        "Record<string, never>".into()
    } else {
        format!("{{ {} }}", parts.join("; "))
    }
}

/// Splits `Either[None, ...]` into the present type and an optional flag.
/// Only [`TsContext::Property`] runs this — it is the fold that turns an
/// `undefined` union member into a `?` on a key.
fn strip_none(kind: &Kind) -> (Kind, bool) {
    let Kind::Either(variants) = kind else {
        return (kind.clone(), false);
    };
    let present: Vec<Kind> = variants
        .iter()
        .filter(|variant| !matches!(variant, Kind::None))
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

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// Renders the named-parameter object for a query from its inferred
/// parameters, applying `OneOf` domains as literal unions. `__hostN`
/// substitution parameters are excluded — they type the subs tuple.
///
/// This is an object type, but **not** a [`TsContext::Property`] position:
/// the `?` here answers "does the query need this argument at all" —
/// `required` is cleared only by a `DEFINE PARAM` default — while a
/// `Property`'s `?` answers "can the value be absent". They are different
/// questions with the same syntax, so the param's kind is rendered as a
/// [`TsContext::Value`] and the caller keeps its own marker. Folding a
/// param's `option<T>` into the key as well would change the type (it would
/// let callers omit a key the query requires), not just its spelling.
pub fn params_type(params: &[ParamInference]) -> String {
    let mut parts = Vec::new();
    for param in params {
        if param.name.starts_with("__host") {
            continue;
        }
        let marker = if param.required { "" } else { "?" };
        parts.push(format!(
            "{}{marker}: {}",
            param.name,
            param_value_type(param)
        ));
    }
    if parts.is_empty() {
        "Record<string, never>".into()
    } else {
        format!("{{ {} }}", parts.join("; "))
    }
}

/// The substitution tuple: the constrained type of each `${...}` in
/// template order. Every element is a [`TsContext::Value`] — a tuple slot has
/// no key, and an omitted element would shorten the tuple.
pub fn subs_tuple(params: &[ParamInference]) -> String {
    let mut hosts: Vec<&ParamInference> = params
        .iter()
        .filter(|param| param.name.starts_with("__host"))
        .collect();
    hosts.sort_by_key(|param| {
        param.name["__host".len()..]
            .parse::<usize>()
            .unwrap_or(usize::MAX)
    });
    let items: Vec<String> = hosts.iter().map(|param| param_value_type(param)).collect();
    format!("[{}]", items.join(", "))
}

fn param_value_type(param: &ParamInference) -> String {
    if let Some(ValueDomain::OneOf(values)) = &param.domain {
        let mut literals: Vec<String> = values
            .iter()
            .map(|value| match value {
                surrealdb_types::Value::String(text) => {
                    format!("\"{}\"", text.replace('"', "\\\""))
                }
                other => ts_value_fallback(other),
            })
            .collect();
        literals.dedup();
        if !literals.is_empty() {
            return literals.join(" | ");
        }
    }
    param.kind.as_ref().map_or_else(
        || "unknown".into(),
        |kind| ts_type(kind, TsContext::Value).text,
    )
}

fn ts_value_fallback(value: &surrealdb_types::Value) -> String {
    match value {
        surrealdb_types::Value::Number(number) => number.to_string(),
        surrealdb_types::Value::Bool(flag) => flag.to_string(),
        _ => "unknown".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// The value spelling, which is what all but one position wants.
    fn value(kind: &Kind) -> String {
        ts_type(kind, TsContext::Value).text
    }

    #[test]
    fn kinds_render_to_typescript() {
        assert_eq!(value(&Kind::String), "string");
        assert_eq!(value(&Kind::Datetime), "Date");
        assert_eq!(
            value(&Kind::Array(Box::new(Kind::Int), None)),
            "Array<number>"
        );
        assert_eq!(
            value(&Kind::Record(vec!["person".into()])),
            "RecordId<\"person\">"
        );
        assert_eq!(
            value(&Kind::Either(vec![Kind::String, Kind::Int])),
            "string | number"
        );
        assert_eq!(
            value(&Kind::Literal(KindLiteral::String("active".into()))),
            "\"active\""
        );
    }

    /// The SDK hands back its own value classes, so the generated types must
    /// name them. Verified against `surrealdb@2.0.8`: a CBOR round-trip of each
    /// of these decodes to the class, not to a string or a number, and a
    /// `RecordId` param encodes to a record link (tag 8) where a plain string
    /// encodes to a SurrealQL string — so `WHERE team = $team` only matches
    /// when the param is a `RecordId`.
    #[test]
    fn sdk_value_classes_render_as_their_classes() {
        assert_eq!(value(&Kind::Uuid), "Uuid");
        assert_eq!(value(&Kind::Duration), "Duration");
        assert_eq!(value(&Kind::Decimal), "Decimal");
        assert_eq!(
            value(&Kind::Record(vec!["team".into()])),
            "RecordId<\"team\">"
        );
        // `datetime` stays `Date`: `createClient` sets
        // `codecOptions.useNativeDates`, which makes that true.
        assert_eq!(value(&Kind::Datetime), "Date");
        // `int`/`float` really are JS numbers; only `decimal` is not.
        assert_eq!(value(&Kind::Int), "number");
        assert_eq!(value(&Kind::Float), "number");
    }

    #[test]
    fn object_literals_render_with_optional_none_fields() {
        let mut fields = BTreeMap::new();
        fields.insert("name".to_string(), Kind::String);
        fields.insert(
            "nick".to_string(),
            Kind::Either(vec![Kind::None, Kind::String]),
        );
        let kind = Kind::Literal(KindLiteral::Object(fields));

        assert_eq!(value(&kind), "{ name: string; nick?: string }");
    }

    /// The split this whole context exists for: one kind, two positions, two
    /// spellings that are *not* the same TypeScript type — `{ nick?: string }`
    /// accepts `{}` and `{ nick: string | undefined }` does not.
    #[test]
    fn one_optional_kind_spells_itself_twice() {
        let optional = Kind::Either(vec![Kind::None, Kind::String]);

        // A value has no key to omit, so the absence is a union member.
        assert_eq!(
            ts_type(&optional, TsContext::Value),
            TsType {
                text: "undefined | string".into(),
                optional: false,
            }
        );
        // A property has one, so the absence moves to the key — and the `?`
        // comes back as a flag, because it belongs before the colon.
        assert_eq!(
            ts_type(&optional, TsContext::Property),
            TsType {
                text: "string".into(),
                optional: true,
            }
        );
    }

    /// A property whose kind cannot be absent still reports `optional: false`,
    /// so `Property` is not "always optional" — it is "fold the `none` if
    /// there is one".
    #[test]
    fn a_property_is_optional_only_when_its_kind_admits_none() {
        assert_eq!(
            ts_type(&Kind::String, TsContext::Property),
            TsType {
                text: "string".into(),
                optional: false,
            }
        );
    }

    /// Only the object member position is a key. Everything a value can be
    /// nested inside — an array element, a tuple slot, a union member — keeps
    /// the `undefined`, because dropping it there would drop the member.
    #[test]
    fn nesting_a_value_never_makes_it_a_property() {
        let optional = Kind::Either(vec![Kind::None, Kind::String]);
        assert_eq!(
            value(&Kind::Array(Box::new(optional.clone()), None)),
            "Array<undefined | string>"
        );
        assert_eq!(
            value(&Kind::Literal(KindLiteral::Array(vec![
                Kind::Int,
                optional.clone(),
            ]))),
            "[number, undefined | string]"
        );
        // …while one level further in, an object member is a key again.
        let mut fields = BTreeMap::new();
        fields.insert("nick".to_string(), optional);
        assert_eq!(
            value(&Kind::Array(
                Box::new(Kind::Literal(KindLiteral::Object(fields))),
                None
            )),
            "Array<{ nick?: string }>"
        );
    }

    #[test]
    fn params_render_named_object_and_subs_tuple() {
        let params = vec![
            ParamInference {
                name: "age".into(),
                kind: Some(Kind::Int),
                domain: None,
                required: true,
                spans: Vec::new(),
            },
            ParamInference {
                name: "__host0".into(),
                kind: Some(Kind::String),
                domain: None,
                required: true,
                spans: Vec::new(),
            },
            ParamInference {
                name: "status".into(),
                kind: Some(Kind::String),
                domain: Some(ValueDomain::OneOf(vec![
                    surrealdb_types::Value::String("open".into()),
                    surrealdb_types::Value::String("closed".into()),
                ])),
                required: false,
                spans: Vec::new(),
            },
        ];

        assert_eq!(
            params_type(&params),
            "{ age: number; status?: \"open\" | \"closed\" }"
        );
        assert_eq!(subs_tuple(&params), "[string]");
    }

    /// The params object looks like a `Property` position and is not one. Its
    /// `?` says the query has a default for the argument; the kind's `none`
    /// says the query accepts a NONE *value*. Folding the second into the
    /// first would let a caller omit a key the query requires — a change of
    /// type, not of spelling — so the kind renders as a value.
    #[test]
    fn a_required_param_that_accepts_none_keeps_its_key() {
        let params = vec![ParamInference {
            name: "nick".into(),
            kind: Some(Kind::Either(vec![Kind::None, Kind::String])),
            domain: None,
            required: true,
            spans: Vec::new(),
        }];

        assert_eq!(params_type(&params), "{ nick: undefined | string }");
    }
}
