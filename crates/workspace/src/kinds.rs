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
    // Prove-or-silent against a scalar *literal* target (`'active'`, `2`):
    // a source that is merely the literal's base kind (`string`, `int`)
    // carries no evidence about *which* value it holds, so it can never be
    // proven to fall outside the target. Widening it to the base is what
    // makes `DEFINE FIELD st TYPE 'active' | 'inactive'` writable at all —
    // inference types a written `'active'` as `string`, so without this
    // every valid write to a literal-union field is a false 2001.
    //
    // A *literal* source is the opposite case: its value is known, so
    // `'crimson'` into `'marble' | 'euclid'` is a provable mismatch and
    // must stay a finding. It falls through to the base reduction below,
    // which compares the two literals' bases and then fails the equality
    // test — so this rule is forward-compatible: the day a call site hands
    // in a literal-kinded value, the exact-value check comes back for free.
    //
    // Restricted to scalar literals: object/array literal targets carry
    // real structure that a bare `object`/`array` source would erase, and
    // that structural comparison is handled above.
    if let Some(expected_base) = scalar_literal_base_kind(expected) {
        // A literal source that did not match `expected` exactly at the top
        // is a *different* known value: provably wrong. Reducing it to its
        // base here would make every string match every string literal.
        if literal_base_kind(actual).is_some() {
            return false;
        }
        return kind_is_assignable_to(actual, &expected_base);
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

/// The base kind of a *scalar* literal kind (`'active'` -> `string`,
/// `2` -> `int`). Object and array literals are excluded: their base
/// (`object` / `array<...>`) throws away the structure that assignability
/// compares, so they are never widened.
fn scalar_literal_base_kind(kind: &Kind) -> Option<Kind> {
    match kind {
        Kind::Literal(KindLiteral::Object(_) | KindLiteral::Array(_)) => None,
        _ => literal_base_kind(kind),
    }
}

/// The exact scalar literal kind of a known constant value (`'active'` ->
/// `Kind::Literal("active")`), if the value is a representable scalar.
///
/// Inference widens a written literal to its base (`'active'` reads as
/// `string`), which is right for most positions but loses the one fact a
/// scalar-literal target needs: *which* string it is. A caller that holds a
/// const value can recover the exact kind here, so the equality branch of
/// [`kind_is_assignable_to`] applies instead of the prove-or-silent widening.
pub(crate) fn scalar_value_literal_kind(value: &surrealdb_types::Value) -> Option<Kind> {
    use surrealdb_types::{KindLiteral, Number, Value};
    let literal = match value {
        Value::String(text) => KindLiteral::String(text.clone()),
        Value::Bool(flag) => KindLiteral::Bool(*flag),
        Value::Duration(duration) => KindLiteral::Duration(*duration),
        Value::Number(Number::Int(int)) => KindLiteral::Integer(*int),
        Value::Number(Number::Float(float)) => KindLiteral::Float(*float),
        Value::Number(Number::Decimal(decimal)) => KindLiteral::Decimal(*decimal),
        // NONE/NULL are their own kinds, not literals; everything else
        // (datetime, uuid, records, collections) has no scalar literal kind.
        _ => return None,
    };
    Some(Kind::Literal(literal))
}

/// Whether `kind` constrains a value to specific scalar literals — either a
/// bare `'active'` or a union like `'active' | 'inactive'`. This is the only
/// shape for which knowing a written value's *exact* literal changes the
/// assignability verdict, so callers use it to bound when they substitute
/// [`scalar_value_literal_kind`] for the widened inferred kind.
pub(crate) fn constrains_scalar_literals(kind: &Kind) -> bool {
    match kind {
        Kind::Literal(_) => scalar_literal_base_kind(kind).is_some(),
        Kind::Either(variants) => variants.iter().any(constrains_scalar_literals),
        _ => false,
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

/// One layer peeled off a kind by [`peel_wrappers`], outermost first.
///
/// A wrapper carries no information about *what* it wraps, so the same list
/// re-applies to whatever a traversal resolves on the far side of the payload
/// — that is how `option<record<user>>.name` keeps its optionality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KindWrapper {
    /// An `option<...>`: an `Either` of exactly one payload arm plus at least
    /// one `NONE`/`NULL` arm.
    Optional,
    Array(Option<u64>),
    Set(Option<u64>),
}

/// Splits `kind` into the `option`/`array`/`set` layers around it and the
/// single payload kind underneath (`option<array<record<user>>>` →
/// `[Optional, Array(None)]` + `record<user>`). A bare kind peels to an empty
/// wrapper list and itself.
///
/// A union with more than one non-`NONE` arm is *not* peeled: there is no
/// single payload to traverse, so the caller keeps its conservative answer
/// rather than picking an arm. The payload is returned as that whole `Either`,
/// which no caller will recognise as a record link.
pub(crate) fn peel_wrappers(kind: &Kind) -> (Vec<KindWrapper>, Kind) {
    let mut wrappers = Vec::new();
    let mut current = kind.clone();
    loop {
        let next = match &current {
            Kind::Array(inner, len) => {
                wrappers.push(KindWrapper::Array(*len));
                (**inner).clone()
            }
            Kind::Set(inner, len) => {
                wrappers.push(KindWrapper::Set(*len));
                (**inner).clone()
            }
            Kind::Either(variants) => {
                let mut payload = variants
                    .iter()
                    .filter(|variant| !matches!(variant, Kind::None | Kind::Null));
                let Some(only) = payload.next().cloned() else {
                    break;
                };
                if payload.next().is_some() {
                    break;
                }
                // `Either([T])` (no NONE arm) is just `T` — no optionality to
                // record, but still worth stepping into.
                if variants.len() > 1 {
                    wrappers.push(KindWrapper::Optional);
                }
                only
            }
            _ => break,
        };
        current = next;
    }
    (wrappers, current)
}

/// Re-applies the layers [`peel_wrappers`] removed, innermost last:
/// `[Optional, Array(None)]` + `string` → `option<array<string>>`.
pub(crate) fn rewrap_kind(wrappers: &[KindWrapper], inner: Kind) -> Kind {
    wrappers
        .iter()
        .rev()
        .fold(inner, |acc, wrapper| match wrapper {
            KindWrapper::Optional => Kind::either(vec![Kind::None, acc]),
            KindWrapper::Array(len) => Kind::Array(Box::new(acc), *len),
            KindWrapper::Set(len) => Kind::Set(Box::new(acc), *len),
        })
}

/// The linked tables of a kind that is a record link under any number of
/// `option`/`array`/`set` wrappers, together with those wrappers.
///
/// `record<user>` → `([], [user])`; `option<record<user>>` →
/// `([Optional], [user])`; `array<record<user>>` → `([Array], [user])`.
/// Anything whose payload is not a `record<...>` — including a union with two
/// unrelated arms — is not a link, so callers stay conservative.
pub(crate) fn record_link_shape(
    kind: &Kind,
) -> Option<(Vec<KindWrapper>, Vec<surrealdb_types::Table>)> {
    match peel_wrappers(kind) {
        (wrappers, Kind::Record(targets)) => Some((wrappers, targets)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Sub-path refinement (a descendant `DEFINE FIELD` narrows its parent's kind)
// ---------------------------------------------------------------------------

/// Narrows `parent` so the sub-path `steps` under it has kind `child`, or
/// `None` when the parent's declared kind admits no such sub-path.
///
/// This is the "descendants REFINE, never replace" rule behind
/// `DEFINE FIELD items TYPE array<object>` + `DEFINE FIELD items[*].price
/// TYPE string` → `array<{ price: string }>`. Three properties matter:
///
/// * The parent's own shape survives. `array<...>`/`set<...>` stay collections
///   and the refinement lands on the ELEMENT; a literal object keeps the
///   siblings it already declared.
/// * Optionality survives. `option<object>` + a subfield stays optional — a
///   subfield declaration never makes its parent required, so
///   `SET parent = NONE` keeps type-checking.
/// * A bare `object` opens into a closed literal object, which is exactly what
///   the nested-field prefix synthesis already produced.
pub(crate) fn refine_subkind(
    parent: &Kind,
    steps: &[crate::schema::FieldStep],
    child: &Kind,
) -> Option<Kind> {
    use crate::schema::FieldStep;

    let Some((step, rest)) = steps.split_first() else {
        return Some(child.clone());
    };
    match parent {
        // A union: refine every payload arm and keep the `NONE`/`NULL` arms
        // untouched, so `option<T>` refines to `option<T'>`.
        Kind::Either(variants) => {
            let mut refined = Vec::with_capacity(variants.len());
            let mut touched = false;
            for variant in variants {
                if matches!(variant, Kind::None | Kind::Null) {
                    refined.push(variant.clone());
                    continue;
                }
                refined.push(refine_subkind(variant, steps, child)?);
                touched = true;
            }
            touched.then(|| Kind::either(refined))
        }
        // A collection refines its element. A `Field` step here means the
        // declaration wrote `items.price` where `items[*].price` was meant —
        // SurrealDB's own idiom flattening reads it the same way, so take the
        // element step implicitly rather than rejecting a real schema.
        Kind::Array(element, len) => {
            let tail = if matches!(step, FieldStep::Element) {
                rest
            } else {
                steps
            };
            Some(Kind::Array(
                Box::new(refine_subkind(element, tail, child)?),
                *len,
            ))
        }
        Kind::Set(element, len) => {
            let tail = if matches!(step, FieldStep::Element) {
                rest
            } else {
                steps
            };
            Some(Kind::Set(
                Box::new(refine_subkind(element, tail, child)?),
                *len,
            ))
        }
        // An open object closes into a literal carrying just this subfield;
        // later siblings refine the literal.
        Kind::Object | Kind::Any => match step {
            FieldStep::Field(name) => {
                let mut fields = BTreeMap::new();
                fields.insert(name.clone(), refine_subkind(&Kind::Any, rest, child)?);
                Some(Kind::Literal(KindLiteral::Object(fields)))
            }
            // `any` is open enough to be a collection too; a bare `object`
            // is not.
            FieldStep::Element if matches!(parent, Kind::Any) => Some(Kind::Array(
                Box::new(refine_subkind(&Kind::Any, rest, child)?),
                None,
            )),
            FieldStep::Element => None,
        },
        Kind::Literal(KindLiteral::Object(fields)) => match step {
            FieldStep::Field(name) => {
                let current = fields.get(name).cloned().unwrap_or(Kind::Any);
                let mut fields = fields.clone();
                fields.insert(name.clone(), refine_subkind(&current, rest, child)?);
                Some(Kind::Literal(KindLiteral::Object(fields)))
            }
            FieldStep::Element => None,
        },
        // Everything else — a scalar, a record link, a geometry — has no
        // sub-path to refine.
        _ => None,
    }
}

/// The kind at the sub-path `steps` under `parent`, or `None` when the parent's
/// declared kind proves no such sub-path exists. The read-only counterpart of
/// [`refine_subkind`]: it never invents a member, so `Some` means "the schema
/// declares this".
pub(crate) fn subkind_at(parent: &Kind, steps: &[crate::schema::FieldStep]) -> Option<Kind> {
    use crate::schema::FieldStep;

    let Some((step, rest)) = steps.split_first() else {
        return Some(parent.clone());
    };
    match parent {
        Kind::Either(variants) => {
            let mut payload = variants
                .iter()
                .filter(|variant| !matches!(variant, Kind::None | Kind::Null));
            let only = payload.next()?;
            if payload.next().is_some() {
                return None;
            }
            let resolved = subkind_at(only, steps)?;
            Some(if variants.len() > 1 {
                Kind::either(vec![Kind::None, resolved])
            } else {
                resolved
            })
        }
        Kind::Array(element, _) | Kind::Set(element, _) => {
            let tail = if matches!(step, FieldStep::Element) {
                rest
            } else {
                steps
            };
            subkind_at(element, tail)
        }
        Kind::Literal(KindLiteral::Object(fields)) => match step {
            FieldStep::Field(name) => subkind_at(fields.get(name)?, rest),
            FieldStep::Element => None,
        },
        _ => None,
    }
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
    fn a_base_kind_fits_a_literal_union_but_a_wrong_literal_or_base_does_not() {
        let status = Kind::Either(vec![string_literal("active"), string_literal("inactive")]);
        let level = Kind::Either(vec![
            Kind::Literal(KindLiteral::Integer(1)),
            Kind::Literal(KindLiteral::Integer(2)),
        ]);

        // The false-2001 case: inference widens a written `'active'` to
        // `string`, which carries no evidence of falling outside the union.
        assert!(kind_is_assignable_to(&Kind::String, &status));
        assert!(kind_is_assignable_to(&Kind::Int, &level));
        assert!(kind_is_assignable_to(&Kind::String, &string_literal("active")));

        // Must-still-fail boundaries. A wrong *base* is a provable mismatch.
        assert!(!kind_is_assignable_to(&Kind::Int, &status));
        assert!(!kind_is_assignable_to(&Kind::String, &level));
        assert!(!kind_is_assignable_to(&Kind::None, &status));
        assert!(!kind_is_assignable_to(&Kind::Float, &level));
        // And a *known* value outside the union stays a mismatch, so the
        // exact-value check returns the moment a call site supplies one.
        assert!(!kind_is_assignable_to(&string_literal("bogus"), &status));
        assert!(!kind_is_assignable_to(
            &Kind::Literal(KindLiteral::Integer(9)),
            &level
        ));
        // Widening never applies to structured literal targets: a bare
        // `object` must not satisfy a structured object type.
        assert!(!kind_is_assignable_to(
            &Kind::Object,
            &object(&[("theme", Kind::String)])
        ));
    }

    /// The rendered codes a query produces end to end.
    fn codes(query: &str) -> Vec<String> {
        let mut workspace = crate::analysis::Workspace::default();
        crate::analysis::analyze_query(&mut workspace, query)
            .diagnostics
            .iter()
            .map(|finding| finding.code().to_string())
            .collect()
    }

    #[test]
    fn writes_to_a_literal_union_field_are_not_false_type_errors() {
        // Every write here is valid SurrealQL; a 2001 on any of them aborts
        // `generate` for the entire workspace.
        for query in [
            "DEFINE FIELD st ON t TYPE 'active' | 'inactive';\nCREATE t SET st = 'active';",
            "DEFINE FIELD st ON t TYPE 'active' | 'inactive';\nCREATE t CONTENT { st: 'active' };",
            "DEFINE FIELD st ON t TYPE 'active' | 'inactive';\nUPDATE t SET st = 'inactive';",
            "DEFINE FIELD lvl ON t TYPE 1 | 2 | 3;\nCREATE t SET lvl = 2;",
        ] {
            assert!(
                !codes(query).iter().any(|code| code == "E2001"),
                "codes for {query:?}: {:?}",
                codes(query)
            );
        }
    }

    #[test]
    fn a_wrong_typed_write_to_a_literal_union_field_still_fires() {
        // The must-still-fire boundary: an `int` cannot inhabit a
        // string-literal union no matter which string it turns out to be.
        for query in [
            "DEFINE FIELD st ON t TYPE 'active' | 'inactive';\nCREATE t SET st = 42;",
            "DEFINE FIELD st ON t TYPE 'active' | 'inactive';\nCREATE t CONTENT { st: 42 };",
            "DEFINE FIELD lvl ON t TYPE 1 | 2 | 3;\nCREATE t SET lvl = 'two';",
        ] {
            assert!(
                codes(query).iter().any(|code| code == "E2001"),
                "codes for {query:?}: {:?}",
                codes(query)
            );
        }
    }

    #[test]
    fn a_wrong_literal_written_to_a_literal_union_field_fires() {
        // The value is the *right base kind* but the wrong literal. Widening
        // alone can't catch this (a `string` can't be shown to fall outside a
        // string-literal union), so the write site recovers the constant's
        // exact literal kind — otherwise every misspelled status slips through.
        for query in [
            "DEFINE FIELD st ON t TYPE 'active' | 'inactive';\nCREATE t SET st = 'bogus';",
            "DEFINE FIELD st ON t TYPE 'active' | 'inactive';\nUPDATE t SET st = 'Active';",
            "DEFINE FIELD lvl ON t TYPE 1 | 2 | 3;\nCREATE t SET lvl = 9;",
        ] {
            assert!(
                codes(query).iter().any(|code| code == "E2001"),
                "codes for {query:?}: {:?}",
                codes(query)
            );
        }
    }

    #[test]
    fn recovering_an_exact_literal_does_not_constrain_plain_fields() {
        // The narrowing is bounded to literal-constrained targets: a plain
        // `string` field still accepts any string, and a non-constant value
        // (a param) is unaffected in both cases.
        for query in [
            "DEFINE FIELD t ON x TYPE string;\nCREATE x SET t = 'anything at all';",
            "DEFINE FIELD st ON x TYPE 'active' | 'inactive';\nCREATE x SET st = $status;",
            "DEFINE FIELD n ON x TYPE int;\nCREATE x SET n = 7;",
        ] {
            assert!(
                !codes(query).iter().any(|code| code == "E2001"),
                "codes for {query:?}: {:?}",
                codes(query)
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

    fn user_link() -> Kind {
        Kind::Record(vec![Table::from("user")])
    }

    fn option_of(inner: Kind) -> Kind {
        Kind::Either(vec![Kind::None, inner])
    }

    #[test]
    fn wrappers_peel_outermost_first_and_rewrap_to_the_original() {
        let cases: &[(&str, Kind, Vec<KindWrapper>)] = &[
            ("bare", user_link(), vec![]),
            ("option", option_of(user_link()), vec![KindWrapper::Optional]),
            (
                "array",
                Kind::Array(Box::new(user_link()), None),
                vec![KindWrapper::Array(None)],
            ),
            (
                "set",
                Kind::Set(Box::new(user_link()), None),
                vec![KindWrapper::Set(None)],
            ),
            (
                "option of array",
                option_of(Kind::Array(Box::new(user_link()), None)),
                vec![KindWrapper::Optional, KindWrapper::Array(None)],
            ),
            (
                "sized array keeps its bound",
                Kind::Array(Box::new(user_link()), Some(3)),
                vec![KindWrapper::Array(Some(3))],
            ),
        ];
        for (label, kind, expected) in cases {
            let (wrappers, payload) = peel_wrappers(kind);
            assert_eq!(&wrappers, expected, "{label}: wrappers");
            assert_eq!(payload, user_link(), "{label}: payload");
            // Re-applying the wrappers to the payload reconstructs the input,
            // which is what makes `option<record<user>>.name` an
            // `option<string>` rather than a bare `string`.
            assert_eq!(rewrap_kind(&wrappers, payload), *kind, "{label}: rewrap");
        }
    }

    #[test]
    fn a_multi_arm_union_is_not_peeled() {
        // Two real arms: there is no single payload to traverse, so the caller
        // must keep its conservative answer instead of picking an arm.
        let ambiguous = Kind::Either(vec![user_link(), Kind::Int]);
        let (wrappers, payload) = peel_wrappers(&ambiguous);
        assert!(wrappers.is_empty());
        assert_eq!(payload, ambiguous);
        assert!(record_link_shape(&ambiguous).is_none());

        // …and neither is a union of two *different* collections.
        let mixed = Kind::Either(vec![
            Kind::Array(Box::new(user_link()), None),
            Kind::Array(Box::new(Kind::Int), None),
        ]);
        assert!(record_link_shape(&mixed).is_none());
    }

    #[test]
    fn record_link_shape_reports_the_targets_under_any_wrapping() {
        let targets = vec![Table::from("user")];
        assert_eq!(
            record_link_shape(&option_of(user_link())),
            Some((vec![KindWrapper::Optional], targets.clone()))
        );
        assert_eq!(
            record_link_shape(&Kind::Array(Box::new(user_link()), None)),
            Some((vec![KindWrapper::Array(None)], targets))
        );
        // A union link (`record<a | b>`) is one `Kind::Record` with two
        // targets — still a link, and both targets are reported.
        assert_eq!(
            record_link_shape(&option_of(Kind::Record(vec![
                Table::from("a"),
                Table::from("b")
            ]))),
            Some((
                vec![KindWrapper::Optional],
                vec![Table::from("a"), Table::from("b")]
            ))
        );
        // Not a link at all.
        assert!(record_link_shape(&option_of(Kind::String)).is_none());
        assert!(record_link_shape(&Kind::Any).is_none());
    }

    #[test]
    fn refining_a_sub_path_preserves_the_parent_shape() {
        use crate::schema::FieldStep::{Element, Field};

        let named = |name: &str| Field(name.to_string());

        // A collection stays a collection; the refinement lands on the element.
        assert_eq!(
            refine_subkind(
                &Kind::Array(Box::new(Kind::Object), None),
                &[Element, named("price")],
                &Kind::String
            ),
            Some(Kind::Array(
                Box::new(object(&[("price", Kind::String)])),
                None
            ))
        );
        // A bare `array` gains an element type from its `[*]` declaration.
        assert_eq!(
            refine_subkind(
                &Kind::Array(Box::new(Kind::Any), None),
                &[Element],
                &Kind::Object
            ),
            Some(Kind::Array(Box::new(Kind::Object), None))
        );
        // Optionality survives: a subfield never makes its parent required.
        assert_eq!(
            refine_subkind(
                &Kind::either(vec![Kind::None, Kind::Object]),
                &[named("theme")],
                &Kind::String
            ),
            Some(Kind::either(vec![
                Kind::None,
                object(&[("theme", Kind::String)])
            ]))
        );
        // A literal object keeps the siblings it already declared.
        assert_eq!(
            refine_subkind(
                &object(&[("name", Kind::String)]),
                &[named("age")],
                &Kind::Int
            ),
            Some(object(&[("age", Kind::Int), ("name", Kind::String)]))
        );
        // A scalar has no sub-path at all (this is what 1025 reports), and a
        // bare object has no ELEMENT.
        assert!(refine_subkind(&Kind::String, &[named("sub")], &Kind::String).is_none());
        assert!(refine_subkind(&Kind::Object, &[Element], &Kind::String).is_none());
        assert!(refine_subkind(&Kind::Datetime, &[Element], &Kind::String).is_none());
    }

    #[test]
    fn reading_a_sub_path_only_reports_what_the_kind_declares() {
        use crate::schema::FieldStep::{Element, Field};

        let named = |name: &str| Field(name.to_string());
        let kind = Kind::Array(Box::new(object(&[("sku", Kind::String)])), None);

        assert_eq!(
            subkind_at(&kind, &[Element, named("sku")]),
            Some(Kind::String)
        );
        // A `Field` step into a collection reads as the element step SurrealQL
        // idiom flattening implies.
        assert_eq!(subkind_at(&kind, &[named("sku")]), Some(Kind::String));
        assert!(subkind_at(&kind, &[named("ghost")]).is_none());
        // An open object proves nothing about its members, so it never claims
        // one exists.
        assert!(subkind_at(&Kind::Object, &[named("anything")]).is_none());
    }
}
