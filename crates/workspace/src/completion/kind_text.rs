//! Reading a [`Kind`] back out of the documented type text carried by the
//! built-in catalog.
//!
//! The built-in signatures in [`super::builtins`] are prose extracted from the
//! analyzer's own module docs (`string::len(string) -> int`), not structured
//! data. Completion needs them as kinds so a built-in can be ranked by whether
//! its return type fits the position, and so a call argument can supply an
//! expected kind. Anything the grammar below does not recognize resolves to
//! `None`, which ranking treats as "type unknown" — never as a mismatch.

use surrealdb_types::Kind;

/// The kind of the `index`-th parameter in a documented parameter list.
///
/// Handles the shapes the catalog actually contains: `array, value`,
/// `path: string[, options]`, `min?, max?`, `(Optional<int>, Optional<int>)`,
/// and a trailing `...` for a variadic tail (whose kind repeats).
pub(crate) fn parameter_kind(params: &str, index: usize) -> Option<Kind> {
    let params = params.trim().trim_start_matches('(').trim_end_matches(')');
    let parts = split_top_level(params);
    if parts.is_empty() {
        return None;
    }
    let part = parts
        .get(index)
        .or_else(|| parts.last().filter(|last| last.trim() == "..."))
        .or_else(|| {
            // A variadic tail (`a, b, ...`) repeats the kind before the `...`.
            parts
                .last()
                .filter(|last| last.trim() == "...")
                .and(parts.get(parts.len().saturating_sub(2)))
        })?;
    parse(part)
}

/// The kind a documented return type names, or `None` when the text is not a
/// type at all (`element`, `accumulated`, `a | b | ...`).
pub(crate) fn return_kind(returns: &str) -> Option<Kind> {
    parse(returns)
}

/// Parses one documented type. Recognizes the base kinds, the
/// `option`/`array`/`set`/`record`/`geometry` wrappers, and `|` unions.
fn parse(text: &str) -> Option<Kind> {
    let text = text.trim();
    // `name: kind` — the documented parameter name is decoration.
    let text = match text.split_once(": ") {
        Some((_, rest)) => rest.trim(),
        None => text,
    };
    // `string[, options]` — the bracket notation marks the tail as optional;
    // only the part before it names this parameter's type.
    let text = text.split('[').next().unwrap_or(text).trim();
    // `kind?` — an optional parameter is still that kind.
    let text = text.trim_end_matches('?').trim();
    if text.is_empty() {
        return None;
    }

    let arms = split_unions(text);
    if arms.len() > 1 {
        let parsed: Option<Vec<Kind>> = arms.iter().map(|arm| parse(arm)).collect();
        return parsed.map(Kind::either);
    }

    if let Some((head, inner)) = split_generic(text) {
        let head = head.to_ascii_lowercase();
        return match head.as_str() {
            "option" | "optional" => Some(Kind::option(parse(inner)?)),
            "array" => Some(Kind::Array(Box::new(parse(inner)?), None)),
            "set" => Some(Kind::Set(Box::new(parse(inner)?), None)),
            "record" => Some(Kind::Record(
                split_unions(inner)
                    .iter()
                    .map(|name| surrealdb_types::Table::from(name.trim()))
                    .collect(),
            )),
            // `geometry<point>` and friends stay a geometry; the concrete
            // shape is not something ranking distinguishes.
            "geometry" => Some(Kind::Geometry(Vec::new())),
            "file" => Some(Kind::File(Vec::new())),
            _ => None,
        };
    }

    Some(match text.to_ascii_lowercase().as_str() {
        "any" | "value" => Kind::Any,
        "bool" | "boolean" => Kind::Bool,
        "bytes" => Kind::Bytes,
        "datetime" => Kind::Datetime,
        "decimal" => Kind::Decimal,
        "duration" => Kind::Duration,
        "float" => Kind::Float,
        "int" | "integer" => Kind::Int,
        "number" => Kind::Number,
        "object" => Kind::Object,
        "string" => Kind::String,
        "uuid" => Kind::Uuid,
        "regex" => Kind::Regex,
        "range" => Kind::Range,
        "null" => Kind::Null,
        "none" => Kind::None,
        "array" => Kind::Array(Box::new(Kind::Any), None),
        "set" => Kind::Set(Box::new(Kind::Any), None),
        "record" => Kind::Record(Vec::new()),
        "table" => Kind::Table(Vec::new()),
        "geometry" | "point" | "line" | "polygon" => Kind::Geometry(Vec::new()),
        "file" => Kind::File(Vec::new()),
        _ => return None,
    })
}

/// Splits on commas that are not inside `<>`, `()`, or `[]`.
fn split_top_level(text: &str) -> Vec<&str> {
    split_on(text, ',')
}

/// Splits a union on `|` at the top level.
fn split_unions(text: &str) -> Vec<&str> {
    split_on(text, '|')
}

fn split_on(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (index, character) in text.char_indices() {
        match character {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            _ if character == separator && depth <= 0 => {
                parts.push(text[start..index].trim());
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() || !parts.is_empty() {
        parts.push(tail);
    }
    parts.retain(|part| !part.is_empty());
    parts
}

/// `array<string>` → `("array", "string")`.
fn split_generic(text: &str) -> Option<(&str, &str)> {
    let open = text.find('<')?;
    let close = text.rfind('>')?;
    (close > open).then(|| (&text[..open], &text[open + 1..close]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_kinds_round_trip_from_their_documented_names() {
        assert_eq!(return_kind("int"), Some(Kind::Int));
        assert_eq!(return_kind("bool"), Some(Kind::Bool));
        assert_eq!(return_kind("string"), Some(Kind::String));
        assert_eq!(return_kind("datetime"), Some(Kind::Datetime));
    }

    #[test]
    fn wrappers_and_unions_parse_structurally() {
        assert_eq!(
            return_kind("array<string>"),
            Some(Kind::Array(Box::new(Kind::String), None))
        );
        assert_eq!(
            return_kind("option<string>"),
            Some(Kind::option(Kind::String))
        );
        assert_eq!(
            return_kind("record<person>"),
            Some(Kind::Record(vec![surrealdb_types::Table::from("person")]))
        );
        assert_eq!(
            return_kind("string | int"),
            Some(Kind::either(vec![Kind::String, Kind::Int]))
        );
    }

    #[test]
    fn prose_that_names_no_type_resolves_to_nothing_rather_than_a_wrong_kind() {
        // These really appear in the catalog; ranking must read them as
        // "unknown", never as a mismatch.
        assert_eq!(return_kind("element"), None);
        assert_eq!(return_kind("accumulated"), None);
        assert_eq!(return_kind("a | b | ..."), None);
        assert_eq!(return_kind(""), None);
    }

    #[test]
    fn parameter_lists_index_positionally_and_strip_the_documented_decoration() {
        assert_eq!(
            parameter_kind("array, value", 0),
            Some(Kind::Array(Box::new(Kind::Any), None))
        );
        assert_eq!(parameter_kind("string, string", 1), Some(Kind::String));
        assert_eq!(
            parameter_kind("path: string[, options]", 0),
            Some(Kind::String)
        );
        assert_eq!(parameter_kind("min?, max?", 0), None);
        assert_eq!(
            parameter_kind("(Optional<int>, Optional<int>)", 1),
            Some(Kind::option(Kind::Int))
        );
        assert_eq!(parameter_kind("datetime?", 0), Some(Kind::Datetime));
        // Out of range with no variadic tail: no expectation.
        assert_eq!(parameter_kind("string", 3), None);
    }
}
