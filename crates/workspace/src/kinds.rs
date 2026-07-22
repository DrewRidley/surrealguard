//! Kind relations shared by inference and checking: assignability (the
//! one contract behind 2001 and friends) and literal-kind reduction.

use surrealdb_types::Kind;

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
    // A literal kind is assignable wherever its base kind is: `'active'` is
    // a string, `{ a: int }` is an object.
    if let Some(base) = literal_base_kind(actual) {
        return kind_is_assignable_to(&base, expected);
    }
    matches!(
        (actual, expected),
        (
            Kind::Int | Kind::Float | Kind::Decimal | Kind::Number,
            Kind::Number
        ) | (Kind::Int, Kind::Float | Kind::Decimal)
    )
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
        ];

        for (label, actual, expected, want) in cases {
            assert_eq!(
                kind_is_assignable_to(actual, expected),
                *want,
                "{label}: {actual} into {expected}"
            );
        }
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
