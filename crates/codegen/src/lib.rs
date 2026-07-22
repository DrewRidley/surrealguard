//! TypeScript generation from analysis results.
//!
//! Two layers: [`ts_type`] renders one `surrealdb_types::Kind` as a
//! TypeScript type, and [`render_registry`] emits the generated `.d.ts`
//! — a literal-keyed registry mapping each embedded query to its result
//! type, substitution tuple, and named-parameter object, plus the `surql`
//! tag and `SurqlQuery` carrier the host code consumes.
//!
//! Value conventions (documented in the generated header): datetimes are
//! `Date`, durations/uuids/records are strings (records branded by
//! table), `NONE` is `undefined`, decimals are `number`.

use surrealdb_types::{Kind, KindLiteral};
use surrealguard_workspace::analysis::{ParamInference, ValueDomain};

mod registry;

pub use registry::{render_registry, QueryEntry};

/// Renders a `Kind` as TypeScript.
pub fn ts_type(kind: &Kind) -> String {
    match kind {
        Kind::Any => "unknown".into(),
        Kind::None => "undefined".into(),
        Kind::Null => "null".into(),
        Kind::Bool => "boolean".into(),
        Kind::Int | Kind::Float | Kind::Decimal | Kind::Number => "number".into(),
        Kind::String | Kind::Uuid | Kind::Duration => "string".into(),
        Kind::Datetime => "Date".into(),
        Kind::Bytes => "Uint8Array".into(),
        Kind::Object => "Record<string, unknown>".into(),
        Kind::Array(element, _) | Kind::Set(element, _) => {
            format!("Array<{}>", ts_type(element))
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
            let mut rendered: Vec<String> = variants.iter().map(ts_type).collect();
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
        KindLiteral::Duration(_) => "string".into(),
        KindLiteral::Array(kinds) => {
            let items: Vec<String> = kinds.iter().map(ts_type).collect();
            format!("[{}]", items.join(", "))
        }
        KindLiteral::Object(fields) => object_type(fields.iter()),
    }
}

/// A closed object type. `option<T>`-valued fields (`Either[None, T]`)
/// render as optional properties.
fn object_type<'a>(fields: impl Iterator<Item = (&'a String, &'a Kind)>) -> String {
    let mut parts = Vec::new();
    for (name, kind) in fields {
        let (kind, optional) = strip_none(kind);
        let key = if is_identifier(name) {
            name.clone()
        } else {
            format!("\"{}\"", name.replace('"', "\\\""))
        };
        let marker = if optional { "?" } else { "" };
        parts.push(format!("{key}{marker}: {}", ts_type(&kind)));
    }
    if parts.is_empty() {
        "Record<string, never>".into()
    } else {
        format!("{{ {} }}", parts.join("; "))
    }
}

/// Splits `Either[None, ...]` into the present type and an optional flag.
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
/// template order.
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
    param
        .kind
        .as_ref()
        .map_or_else(|| "unknown".into(), ts_type)
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

    #[test]
    fn kinds_render_to_typescript() {
        assert_eq!(ts_type(&Kind::String), "string");
        assert_eq!(ts_type(&Kind::Datetime), "Date");
        assert_eq!(
            ts_type(&Kind::Array(Box::new(Kind::Int), None)),
            "Array<number>"
        );
        assert_eq!(
            ts_type(&Kind::Record(vec!["person".into()])),
            "RecordId<\"person\">"
        );
        assert_eq!(
            ts_type(&Kind::Either(vec![Kind::String, Kind::Int])),
            "string | number"
        );
        assert_eq!(
            ts_type(&Kind::Literal(KindLiteral::String("active".into()))),
            "\"active\""
        );
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

        assert_eq!(ts_type(&kind), "{ name: string; nick?: string }");
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
}
