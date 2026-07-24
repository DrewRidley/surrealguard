//! Kind relations shared by inference and checking: assignability (the
//! one contract behind 2001 and friends) and literal-kind reduction.

use std::collections::BTreeMap;

use surrealdb_types::{Kind, KindLiteral};

/// Structural assignability between two closed object literals: every source
/// property must name a property the target declares and carry an assignable
/// kind (no extras — a closed object admits none), and every target property
/// the source omits must be optional (`option<...>`), since an omitted
/// property is `NONE`.
fn object_is_assignable_to(src: &BTreeMap<String, Kind>, dst: &BTreeMap<String, Kind>) -> bool {
    for (key, src_kind) in src {
        match dst.get(key) {
            Some(dst_kind) => {
                if !kind_is_assignable_to(src_kind, dst_kind) {
                    return false;
                }
            }
            None => return false,
        }
    }
    dst.iter()
        .all(|(key, dst_kind)| src.contains_key(key) || kind_admits_none(dst_kind))
}

/// Whether a kind accepts `NONE` — an `option<...>` (`none | t`), or the
/// permissive `any`.
fn kind_admits_none(kind: &Kind) -> bool {
    match kind {
        Kind::None | Kind::Null | Kind::Any => true,
        Kind::Either(variants) => variants.iter().any(kind_admits_none),
        _ => false,
    }
}

pub(crate) fn kind_is_assignable_to(actual: &Kind, expected: &Kind) -> bool {
    if matches!(expected, Kind::Any) || actual == expected {
        return true;
    }
    // A union accepts anything one of its variants accepts (`option<t>` is
    // `none | t`); a union value fits only where every variant fits.
    if let Kind::Either(variants) = expected {
        return variants
            .iter()
            .any(|variant| kind_is_assignable_to(actual, variant));
    }
    if let Kind::Either(variants) = actual {
        return variants
            .iter()
            .all(|variant| kind_is_assignable_to(variant, expected));
    }
    // Structural object assignability: an object literal fits an
    // object-typed target when every property the target *requires* is
    // supplied by an assignable source property. Optional (option<>) target
    // properties may be omitted, and a closed target admits no extra source
    // properties. This must run before the literal-base reduction below,
    // which would otherwise collapse `{ a: int }` to a bare `object` and lose
    // the structure. A plain `object` target is open and matched by the base
    // reduction (`object` == `object`).
    if let (
        Kind::Literal(KindLiteral::Object(src)),
        Kind::Literal(KindLiteral::Object(dst)),
    ) = (actual, expected)
    {
        return object_is_assignable_to(src, dst);
    }
    // A literal kind is assignable wherever its base kind is: `'active'` is
    // a string, `{ a: int }` is an object.
    if let Some(base) = literal_base_kind(actual) {
        return kind_is_assignable_to(&base, expected);
    }
    // Collections are covariant in their element and bounded by the target's
    // length. Array and Set stay distinct — SurrealDB never coerces one into
    // the other, so only like-with-like matches here.
    // A record kind is assignable to a record target whose table set is a
    // superset: `record<folder>` fits `record<file | folder>` (a folder
    // record IS a file-or-folder record). An empty target set is `record<>` —
    // any record — and accepts every record; an empty *source* is any record
    // and fits only an equally-unconstrained target.
    if let (Kind::Record(src_tables), Kind::Record(dst_tables)) = (actual, expected) {
        if dst_tables.is_empty() {
            return true;
        }
        return !src_tables.is_empty()
            && src_tables
                .iter()
                .all(|table| dst_tables.contains(table));
    }
    match (actual, expected) {
        (Kind::Array(src_elem, src_len), Kind::Array(dst_elem, dst_len))
        | (Kind::Set(src_elem, src_len), Kind::Set(dst_elem, dst_len)) => {
            // `any` on either side (an empty `[]` infers `array<any, 0>`)
            // makes element covariance vacuous.
            let elements_ok = matches!(**src_elem, Kind::Any)
                || matches!(**dst_elem, Kind::Any)
                || kind_is_assignable_to(src_elem, dst_elem);
            return elements_ok && length_fits(*src_len, *dst_len);
        }
        _ => {}
    }
    matches!(
        (actual, expected),
        (
            Kind::Int | Kind::Float | Kind::Decimal | Kind::Number,
            Kind::Number
        ) | (Kind::Int, Kind::Float | Kind::Decimal)
    )
}

/// Whether a source collection length fits a target's. The target length is
/// an upper bound: a fit fails only when both lengths are known and the source
/// overflows the target's fixed length (an empty `[]` therefore fits any
/// length, and an unbounded target accepts any source).
fn length_fits(src: Option<u64>, dst: Option<u64>) -> bool {
    match (src, dst) {
        (Some(src), Some(dst)) => src <= dst,
        _ => true,
    }
}

/// The base kind a `Kind::Literal` value inhabits, if `kind` is one.
pub(crate) fn literal_base_kind(kind: &Kind) -> Option<Kind> {
    use surrealdb_types::KindLiteral;
    let Kind::Literal(literal) = kind else {
        return None;
    };
    let base = match literal {
        KindLiteral::String(_) => Kind::String,
        KindLiteral::Integer(_) => Kind::Int,
        KindLiteral::Float(_) => Kind::Float,
        KindLiteral::Decimal(_) => Kind::Decimal,
        KindLiteral::Duration(_) => Kind::Duration,
        KindLiteral::Bool(_) => Kind::Bool,
        KindLiteral::Array(kinds) => Kind::Array(
            Box::new(Kind::either(kinds.clone())),
            Some(kinds.len() as u64),
        ),
        KindLiteral::Object(_) => Kind::Object,
    };
    Some(base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealdb_types::{KindLiteral, Table};

    fn string_literal(value: &str) -> Kind {
        Kind::Literal(KindLiteral::String(value.to_string()))
    }

    #[test]
    fn assignability_pins_each_contract_branch() {
        let cases: &[(&str, Kind, Kind, bool)] = &[
            // Exact match and the wildcard target.
            ("exact string", Kind::String, Kind::String, true),
            ("anything into any", Kind::Int, Kind::Any, true),
            // Numeric widening: every number is a Number; Int also fits the
            // wider floating kinds. String is not a number.
            ("int into number", Kind::Int, Kind::Number, true),
            ("float into number", Kind::Float, Kind::Number, true),
            ("int into float", Kind::Int, Kind::Float, true),
            ("int into decimal", Kind::Int, Kind::Decimal, true),
            ("string not into int", Kind::String, Kind::Int, false),
            ("float not into int", Kind::Float, Kind::Int, false),
            // option<string> is Either[None, String]: it accepts either the
            // string or NONE, but nothing else.
            (
                "string into option",
                Kind::String,
                Kind::Either(vec![Kind::None, Kind::String]),
                true,
            ),
            (
                "none into option",
                Kind::None,
                Kind::Either(vec![Kind::None, Kind::String]),
                true,
            ),
            (
                "int not into option-string",
                Kind::Int,
                Kind::Either(vec![Kind::None, Kind::String]),
                false,
            ),
            // A union actual fits only where every one of its variants fits.
            (
                "either-of-numbers into number",
                Kind::Either(vec![Kind::Int, Kind::Float]),
                Kind::Number,
                true,
            ),
            (
                "either-with-string not into number",
                Kind::Either(vec![Kind::Int, Kind::String]),
                Kind::Number,
                false,
            ),
            // Records match by their exact table set.
            (
                "record same table",
                Kind::Record(vec![Table::from("person")]),
                Kind::Record(vec![Table::from("person")]),
                true,
            ),
            (
                "record different table",
                Kind::Record(vec![Table::from("person")]),
                Kind::Record(vec![Table::from("post")]),
                false,
            ),
            // A record fits a target whose table set is a superset: a folder
            // record IS a file-or-folder record.
            (
                "record subset into union target",
                Kind::Record(vec![Table::from("folder")]),
                Kind::Record(vec![Table::from("file"), Table::from("folder")]),
                true,
            ),
            (
                "record union not into narrower target",
                Kind::Record(vec![Table::from("file"), Table::from("folder")]),
                Kind::Record(vec![Table::from("file")]),
                false,
            ),
            (
                "any record target accepts a record",
                Kind::Record(vec![Table::from("file")]),
                Kind::Record(vec![]),
                true,
            ),
            (
                "any record source not into a constrained target",
                Kind::Record(vec![]),
                Kind::Record(vec![Table::from("file")]),
                false,
            ),
            // A literal kind is assignable wherever its base kind is.
            (
                "string literal into string",
                string_literal("active"),
                Kind::String,
                true,
            ),
            (
                "string literal not into int",
                string_literal("active"),
                Kind::Int,
                false,
            ),
            // Collections: covariant element, target length an upper bound,
            // and Array/Set stay distinct.
            (
                "fixed array into unbounded",
                Kind::Array(Box::new(Kind::String), Some(3)),
                Kind::Array(Box::new(Kind::String), None),
                true,
            ),
            (
                "empty array into typed",
                Kind::Array(Box::new(Kind::Any), Some(0)),
                Kind::Array(Box::new(Kind::String), None),
                true,
            ),
            (
                "empty array into fixed length",
                Kind::Array(Box::new(Kind::Any), Some(0)),
                Kind::Array(Box::new(Kind::String), Some(3)),
                true,
            ),
            (
                "array element mismatch",
                Kind::Array(Box::new(Kind::String), Some(2)),
                Kind::Array(Box::new(Kind::Int), None),
                false,
            ),
            (
                "array length overflow",
                Kind::Array(Box::new(Kind::String), Some(4)),
                Kind::Array(Box::new(Kind::String), Some(2)),
                false,
            ),
            (
                "any element target accepts array",
                Kind::Array(Box::new(Kind::String), Some(2)),
                Kind::Array(Box::new(Kind::Any), None),
                true,
            ),
            (
                "int array widens into number array",
                Kind::Array(Box::new(Kind::Int), Some(2)),
                Kind::Array(Box::new(Kind::Number), None),
                true,
            ),
            (
                "set covariance and length bound",
                Kind::Set(Box::new(Kind::Int), Some(2)),
                Kind::Set(Box::new(Kind::Number), None),
                true,
            ),
            (
                "array not into set",
                Kind::Array(Box::new(Kind::String), Some(2)),
                Kind::Set(Box::new(Kind::String), None),
                false,
            ),
        ];

        for (label, actual, expected, want) in cases {
            assert_eq!(
                kind_is_assignable_to(actual, expected),
                *want,
                "{label}: {actual} into {expected}"
            );
        }
    }

    fn object(pairs: &[(&str, Kind)]) -> Kind {
        Kind::Literal(KindLiteral::Object(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        ))
    }

    #[test]
    fn object_literal_assignability_is_structural() {
        let option_string = Kind::Either(vec![Kind::None, Kind::String]);
        let theme_union = Kind::Either(vec![
            string_literal("marble"),
            string_literal("euclid"),
        ]);

        // A string-literal property fits a string-literal-union target, and a
        // missing optional property is fine.
        let target = object(&[
            ("theme", theme_union.clone()),
            ("nickname", option_string.clone()),
        ]);
        let source = object(&[("theme", string_literal("marble"))]);
        assert!(kind_is_assignable_to(&source, &target));

        // A wrong-typed property fails.
        let bad_value = object(&[
            ("theme", string_literal("crimson")),
            ("nickname", Kind::None),
        ]);
        assert!(!kind_is_assignable_to(&bad_value, &target));

        // A missing *required* property fails.
        let required_target = object(&[("theme", theme_union), ("count", Kind::Int)]);
        let missing_required = object(&[("theme", string_literal("euclid"))]);
        assert!(!kind_is_assignable_to(&missing_required, &required_target));

        // An extra property fails against a closed object.
        let extra = object(&[
            ("theme", string_literal("marble")),
            ("nickname", Kind::None),
            ("stray", Kind::Int),
        ]);
        assert!(!kind_is_assignable_to(&extra, &target));

        // Nested objects recurse.
        let nested_target = object(&[("inner", object(&[("flag", Kind::Bool)]))]);
        let nested_source = object(&[(
            "inner",
            object(&[("flag", Kind::Literal(KindLiteral::Bool(true)))]),
        )]);
        assert!(kind_is_assignable_to(&nested_source, &nested_target));
    }

    #[test]
    fn literal_base_kind_reduces_literals_and_ignores_plain_kinds() {
        assert_eq!(literal_base_kind(&string_literal("x")), Some(Kind::String));
        assert_eq!(
            literal_base_kind(&Kind::Literal(KindLiteral::Integer(1))),
            Some(Kind::Int)
        );
        assert_eq!(
            literal_base_kind(&Kind::Literal(KindLiteral::Bool(true))),
            Some(Kind::Bool)
        );
        // A plain (non-literal) kind has no literal base.
        assert_eq!(literal_base_kind(&Kind::String), None);
    }
}
