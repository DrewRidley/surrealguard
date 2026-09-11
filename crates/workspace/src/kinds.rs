//! Kind relations shared by inference and checking: assignability (the
//! one contract behind 2001 and friends) and literal-kind reduction.

use std::collections::BTreeMap;

use surrealdb_types::{GeometryKind, Kind, KindLiteral};

use crate::schema::{FieldStep, SchemaIndex};

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
pub(crate) fn kind_admits_none(kind: &Kind) -> bool {
    match kind {
        Kind::None | Kind::Null | Kind::Any => true,
        Kind::Either(variants) => variants.iter().any(kind_admits_none),
        _ => false,
    }
}

/// Whether a value of `actual` may land where `expected` is required — the one
/// contract behind 2001 and friends, and the subtyping order
/// [`crate::lattice`] is built on.
///
/// Public because it is also the *test* order: the expression-fact migration's
/// both-ways harness asserts that the new narrowing path's kind at every site
/// is assignable to the old path's, which is the mechanical statement of "no
/// precision was lost". Asking that question with a second relation would let
/// the two drift exactly as the refinement transforms did.
pub fn kind_is_assignable_to(actual: &Kind, expected: &Kind) -> bool {
    if matches!(expected, Kind::Any) || actual == expected {
        return true;
    }
    // The two union rules. A union SOURCE fits where every one of its variants
    // fits; a union TARGET accepts whatever any one of its variants accepts
    // (`option<t>` is `none | t`).
    //
    // The source rule has to be asked FIRST, and it has to be asked whatever
    // shape the target has. Testing the target first and returning from that
    // branch asks "does the whole source fit inside ONE target variant" — which
    // no multi-variant source can satisfy, so a sub-union was not assignable to
    // its own superset (`'b' | 'c'` into `'a' | 'b' | 'c'` was a false 2001).
    // Split across the two rules, each source variant gets to pick its own home
    // in the target, which is the standard rule and the one every refinement
    // needs: narrowing a union yields a sub-union of it.
    if let Kind::Either(variants) = actual {
        return variants
            .iter()
            .all(|variant| kind_is_assignable_to(variant, expected));
    }
    if let Kind::Either(variants) = expected {
        return variants
            .iter()
            .any(|variant| kind_is_assignable_to(actual, variant));
    }
    // Geometry. A geometry fits a geometry target that names no shape or
    // names every one of the source's (`geometry<point>` into `geometry`, or
    // into `geometry<point | polygon>`); a GeoJSON object literal (`{ type:
    // 'Point', coordinates: […] }`) IS a geometry on the engine, so it fits
    // under the same rule by the shape its `type` names.
    match (actual, expected) {
        (Kind::Geometry(src), Kind::Geometry(dst)) => {
            return dst.is_empty() || src.iter().all(|shape| dst.contains(shape));
        }
        (Kind::Literal(KindLiteral::Object(src)), Kind::Geometry(dst)) => {
            return geojson_shape(src).is_some_and(|shape| dst.is_empty() || dst.contains(&shape));
        }
        _ => {}
    }
    // Structural object assignability: an object literal fits an
    // object-typed target when every property the target *requires* is
    // supplied by an assignable source property. Optional (option<>) target
    // properties may be omitted, and a closed target admits no extra source
    // properties. This must run before the literal-base reduction below,
    // which would otherwise collapse `{ a: int }` to a bare `object` and lose
    // the structure. A plain `object` target is open and matched by the base
    // reduction (`object` == `object`).
    if let (Kind::Literal(KindLiteral::Object(src)), Kind::Literal(KindLiteral::Object(dst))) =
        (actual, expected)
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
        return !src_tables.is_empty() && src_tables.iter().all(|table| dst_tables.contains(table));
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

/// The geometry shape a GeoJSON object literal spells: a `type` that names one
/// of the seven GeoJSON geometries beside the payload key that shape carries
/// (`coordinates`, or `geometries` for a collection). Anything else is not
/// provably a geometry.
fn geojson_shape(object: &BTreeMap<String, Kind>) -> Option<GeometryKind> {
    let Some(Kind::Literal(KindLiteral::String(name))) = object.get("type") else {
        return None;
    };
    let shape = match name.as_str() {
        "Point" => GeometryKind::Point,
        "LineString" => GeometryKind::Line,
        "Polygon" => GeometryKind::Polygon,
        "MultiPoint" => GeometryKind::MultiPoint,
        "MultiLineString" => GeometryKind::MultiLine,
        "MultiPolygon" => GeometryKind::MultiPolygon,
        "GeometryCollection" => GeometryKind::Collection,
        _ => return None,
    };
    let payload = if shape == GeometryKind::Collection {
        "geometries"
    } else {
        "coordinates"
    };
    object.contains_key(payload).then_some(shape)
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
pub(crate) fn scalar_literal_base_kind(kind: &Kind) -> Option<Kind> {
    match kind {
        Kind::Literal(KindLiteral::Object(_) | KindLiteral::Array(_)) => None,
        _ => literal_base_kind(kind),
    }
}

/// The exact scalar literal kind of a known constant value (`'active'` ->
/// `Kind::Literal("active")`), if the value is a representable scalar.
///
/// Inference widens a written literal to its base (`'active'` reads as
/// `string`), which is right for building a response type and wrong for
/// checking a contract: it loses the one fact a literal target needs, *which*
/// string it is. This is the value-to-kind half of
/// [`crate::analyzer::contract::checked_kind`]; the other half is the folder.
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
pub(crate) fn refine_subkind(parent: &Kind, steps: &[FieldStep], child: &Kind) -> Option<Kind> {
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

/// The kind at the sub-path `steps` under `parent`, read without a schema, or
/// `None` when the parent's declared kind proves no such sub-path exists. The
/// read-only counterpart of [`refine_subkind`], and [`project_path`] with no
/// schema in hand: a record link cannot be entered, so `Some` means "this kind
/// itself declares the member".
pub(crate) fn subkind_at(parent: &Kind, steps: &[FieldStep]) -> Option<Kind> {
    project_path(parent, steps, None)
}

// ---------------------------------------------------------------------------
// Projection (reading one step — a field or an element — out of a kind)
// ---------------------------------------------------------------------------

/// The kind one step reaches out of `kind` — a named field or a collection
/// element — or `None` when `kind` proves no such member exists.
///
/// This is THE projection policy. Every "what is `value.field`" question in
/// the crate — a schema sub-path, an idiom walk, a hover, a destructure, a
/// projected-row oracle — asks it, so the answer cannot drift between sites.
/// The rules, in the order they apply:
///
/// * **`any` projects to `any`.** An unknown value has an unknown member, not
///   a provably absent one.
/// * **A union projects arm by arm.** The `NONE`/`NULL` arms are set aside;
///   every other arm is projected, arms with no such member are dropped, and
///   the survivors are unioned in arm order (order is load-bearing for
///   rendering — see `lattice::canonical_union`). No survivor → `None`. When a
///   sentinel arm was set aside the result is `option<…>` of that union: a
///   value that may be `NONE` has a member that may be `NONE`
///   (`option<record<user>>.name` is `option<string>`). The sentinel comes
///   back as `none`, the shape [`rewrap_kind`] gives [`KindWrapper::Optional`].
/// * **A record link reads the schema.** Each target table contributes what
///   [`crate::analyzer::data::select::kind_for_path`] says the field is
///   (declared, refined, a nested-object prefix, or the implicit
///   `id`/`in`/`out`); tables without the field are dropped and the rest are
///   unioned. A target the schema does not know, an unconstrained `record<>`,
///   or no schema at all (`schema: None`) leaves the member unprovable →
///   `None`. A record has no `Element`.
/// * **A collection distributes.** `Element` is the element kind itself; a
///   `Field` step projects the ELEMENT and wraps the result back in the same
///   collection with the same length (`array<record<user>>.name` is
///   `array<string>`) — SurrealQL's implicit `[*]` on field access.
/// * **A literal object is read by key**; it has no `Element`.
/// * **Everything else** — a scalar, an open `object`, a geometry, a tuple
///   literal, a `table<>` — proves no member: `None`. An open `object` is
///   deliberately *not* `any` here: it says nothing about its keys, so this
///   never invents one, which keeps "does this path exist on the table"
///   checks honest.
pub(crate) fn project(kind: &Kind, step: &FieldStep, schema: Option<&SchemaIndex>) -> Option<Kind> {
    match kind {
        Kind::Any => Some(Kind::Any),
        Kind::Either(variants) => {
            let mut optional = false;
            let mut projected = Vec::with_capacity(variants.len());
            for variant in variants {
                if matches!(variant, Kind::None | Kind::Null) {
                    optional = true;
                    continue;
                }
                if let Some(member) = project(variant, step, schema) {
                    projected.push(member);
                }
            }
            if projected.is_empty() {
                return None;
            }
            let union = Kind::either(projected);
            Some(if optional {
                Kind::either(vec![Kind::None, union])
            } else {
                union
            })
        }
        Kind::Record(targets) => {
            let FieldStep::Field(name) = step else {
                return None;
            };
            let schema = schema?;
            if targets.is_empty() {
                return None;
            }
            // A multi-table link steps into every table at once. An arm that
            // does not declare the field answers NONE — a kind, not an absence
            // of one (3.2.3: `type::of(owner.username)` over a
            // `record<account | organization>` is `['string', 'none']`) — so
            // the read is the union of what the arms answer, and only a field
            // absent on EVERY arm is no field at all (the caller's 1002).
            let mut projected = Vec::with_capacity(targets.len());
            let mut declared = false;
            for target in targets {
                let table = schema.tables.get(&target.to_string())?;
                match crate::analyzer::data::select::kind_for_path(
                    table,
                    std::slice::from_ref(name),
                ) {
                    Some(member) => {
                        declared = true;
                        projected.push(member);
                    }
                    None => projected.push(Kind::None),
                }
            }
            declared.then(|| Kind::either(projected))
        }
        Kind::Array(element, len) => match step {
            FieldStep::Element => Some((**element).clone()),
            FieldStep::Field(_) => {
                Some(Kind::Array(Box::new(project(element, step, schema)?), *len))
            }
        },
        Kind::Set(element, len) => match step {
            FieldStep::Element => Some((**element).clone()),
            FieldStep::Field(_) => Some(Kind::Set(Box::new(project(element, step, schema)?), *len)),
        },
        Kind::Literal(KindLiteral::Object(fields)) => match step {
            FieldStep::Field(name) => fields.get(name).cloned(),
            FieldStep::Element => None,
        },
        _ => None,
    }
}

/// [`project`] applied step after step: the kind at the end of `steps`, or
/// `None` as soon as one step proves no member. An empty path is `kind` itself.
pub(crate) fn project_path(
    kind: &Kind,
    steps: &[FieldStep],
    schema: Option<&SchemaIndex>,
) -> Option<Kind> {
    steps.iter().try_fold(kind.clone(), |current, step| {
        project(&current, step, schema)
    })
}

/// `project` for the common case of a chain of named fields.
pub(crate) fn project_fields(
    kind: &Kind,
    fields: &[String],
    schema: Option<&SchemaIndex>,
) -> Option<Kind> {
    fields.iter().try_fold(kind.clone(), |current, field| {
        project(&current, &FieldStep::Field(field.clone()), schema)
    })
}

/// Whether `kind` is one of the numeric kinds (`int`, `float`, `decimal`, or
/// their supertype `number`). Shared by arithmetic inference, the numeric
/// contracts, and the lattice's coercion-order rule.
pub(crate) fn is_numeric(kind: &Kind) -> bool {
    matches!(kind, Kind::Int | Kind::Float | Kind::Decimal | Kind::Number)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealdb_types::{KindLiteral, Table};

    fn string_literal(value: &str) -> Kind {
        Kind::Literal(KindLiteral::String(value.to_string()))
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one table row per assignability contract branch"
    )]
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
        let theme_union = Kind::Either(vec![string_literal("marble"), string_literal("euclid")]);

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
        assert!(kind_is_assignable_to(
            &Kind::String,
            &string_literal("active")
        ));

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

    /// A union source is assignable to any target that has room for every one
    /// of its variants — the rule that was inverted by asking about an `Either`
    /// TARGET first and returning from that branch, which demanded the whole
    /// source fit inside ONE target variant.
    #[test]
    fn a_sub_union_is_assignable_to_its_superset() {
        let sub = Kind::Either(vec![string_literal("b"), string_literal("c")]);
        let sup = Kind::Either(vec![
            string_literal("a"),
            string_literal("b"),
            string_literal("c"),
        ]);
        // Literal unions: `'b' | 'c'` into `'a' | 'b' | 'c'`.
        assert!(kind_is_assignable_to(&sub, &sup));
        // …and not the other way: an `'a'` has nowhere to go in `'b' | 'c'`.
        assert!(!kind_is_assignable_to(&sup, &sub));
        // A variant outside the superset is still a provable mismatch.
        assert!(!kind_is_assignable_to(
            &Kind::Either(vec![string_literal("b"), string_literal("z")]),
            &sup
        ));

        // Non-literal unions behave the same.
        let ints = Kind::Either(vec![Kind::Int, Kind::String]);
        let wider = Kind::Either(vec![Kind::Int, Kind::String, Kind::Bool]);
        assert!(kind_is_assignable_to(&ints, &wider));
        assert!(!kind_is_assignable_to(&wider, &ints));
        assert!(!kind_is_assignable_to(
            &Kind::Either(vec![Kind::Int, Kind::Datetime]),
            &wider
        ));

        // An `option<T>` is a union with `NONE`, so it fits `option<T | U>`.
        assert!(kind_is_assignable_to(
            &Kind::Either(vec![Kind::None, Kind::String]),
            &Kind::Either(vec![Kind::None, Kind::String, Kind::Int])
        ));
        assert!(!kind_is_assignable_to(
            &Kind::Either(vec![Kind::None, Kind::String, Kind::Int]),
            &Kind::Either(vec![Kind::None, Kind::String])
        ));

        // Equal unions in either order — a union is a set, and its variant
        // sequence is not part of what it admits.
        let forward = Kind::Either(vec![Kind::Int, Kind::String]);
        let reversed = Kind::Either(vec![Kind::String, Kind::Int]);
        assert!(kind_is_assignable_to(&forward, &reversed));
        assert!(kind_is_assignable_to(&reversed, &forward));

        // A record's table set was always a superset check of its own; it must
        // keep working now that unions are handled ahead of it.
        assert!(kind_is_assignable_to(
            &Kind::Record(vec![Table::from("a")]),
            &Kind::Record(vec![Table::from("a"), Table::from("b")])
        ));

        // Collections are covariant, so a collection of a sub-union fits a
        // collection of the superset.
        assert!(kind_is_assignable_to(
            &Kind::Array(Box::new(sub.clone()), None),
            &Kind::Array(Box::new(sup.clone()), None)
        ));
        assert!(!kind_is_assignable_to(
            &Kind::Array(Box::new(sup), None),
            &Kind::Array(Box::new(sub), None)
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
    fn writing_a_sub_union_into_its_superset_field_is_not_a_type_error() {
        // The value's kind is itself a union — a narrower one than the field's.
        // Every value it admits is a value the field admits, so the write is
        // valid and must be silent.
        for query in [
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD f ON t TYPE 'a' | 'b' | 'c';\n\
             DEFINE FUNCTION fn::narrow() -> 'b' | 'c' { RETURN 'b'; };\n\
             CREATE t SET f = fn::narrow();",
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD f ON t TYPE int | string | bool;\n\
             DEFINE FUNCTION fn::narrow() -> int | string { RETURN 1; };\n\
             CREATE t SET f = fn::narrow();",
            // An `option` is a union with NONE, so an option of the payload
            // fits an option of a wider payload.
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD f ON t TYPE option<string | int>;\n\
             DEFINE FUNCTION fn::narrow() -> option<string> { RETURN 'x'; };\n\
             CREATE t SET f = fn::narrow();",
        ] {
            assert!(
                !codes(query).iter().any(|code| code == "E2001"),
                "codes for {query:?}: {:?}",
                codes(query)
            );
        }
    }

    #[test]
    fn writing_a_union_with_a_variant_the_field_rejects_still_fires() {
        // The must-still-fire boundary of the rule above: one variant of the
        // written union has no home in the field's type, so some value the
        // expression can produce is one the field cannot hold.
        for query in [
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD f ON t TYPE 'a' | 'b' | 'c';\n\
             DEFINE FUNCTION fn::wrong() -> 'b' | 'z' { RETURN 'b'; };\n\
             CREATE t SET f = fn::wrong();",
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD f ON t TYPE int | string;\n\
             DEFINE FUNCTION fn::wrong() -> int | datetime { RETURN 1; };\n\
             CREATE t SET f = fn::wrong();",
            // A superset written into a sub-union: the extra variant is exactly
            // the value the field rejects.
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD f ON t TYPE option<string>;\n\
             DEFINE FUNCTION fn::wider() -> option<string | int> { RETURN 'x'; };\n\
             CREATE t SET f = fn::wider();",
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

    /// Whether a query reports the type-mismatch contract.
    fn has_2001(query: &str) -> bool {
        codes(query).iter().any(|code| code == "E2001")
    }

    #[test]
    fn a_wrong_literal_declared_in_a_field_clause_fires() {
        // The DEFINE site is the same contract as the write site: a `VALUE` /
        // `DEFAULT` constant has to inhabit the field's declared type. It used
        // to widen `'green'` to `string` and go silent, so the mistake was
        // caught only once someone wrote the field.
        for query in [
            "DEFINE FIELD e ON t TYPE 'red' | 'blue' VALUE 'green';",
            "DEFINE FIELD e ON t TYPE 'red' | 'blue' DEFAULT 'green';",
            "DEFINE FIELD e ON t TYPE 'red' | 'blue' DEFAULT ALWAYS 'green';",
            "DEFINE FIELD e ON t TYPE option<'red' | 'blue'> VALUE 'green';",
            "DEFINE FIELD n ON t TYPE 1 | 2 | 3 VALUE 9;",
            "DEFINE FIELD n ON t TYPE 1 | 2 | 3 DEFAULT 9;",
        ] {
            assert!(has_2001(query), "codes for {query:?}: {:?}", codes(query));
        }
    }

    #[test]
    fn a_correct_or_unknowable_field_clause_stays_silent() {
        for query in [
            // The right literal fits, so recovering its exact kind must not
            // turn a valid enum field into an error.
            "DEFINE FIELD e ON t TYPE 'red' | 'blue' VALUE 'red';",
            "DEFINE FIELD e ON t TYPE 'red' | 'blue' DEFAULT 'blue';",
            "DEFINE FIELD e ON t TYPE option<'red' | 'blue'> DEFAULT NONE;",
            "DEFINE FIELD n ON t TYPE 1 | 2 | 3 VALUE 2;",
            // Prove-or-stay-silent: a call, a param, and a subquery are not
            // statically known values, so there is nothing to compare against
            // the union and the clause must not be reported.
            "DEFINE FIELD e ON t TYPE 'red' | 'blue' VALUE string::lowercase($value);",
            "DEFINE FIELD e ON t TYPE 'red' | 'blue' DEFAULT $default_colour;",
            "DEFINE FIELD e ON t TYPE 'red' | 'blue' VALUE (SELECT VALUE name FROM ONLY t LIMIT 1);",
            // A plain field is unaffected: no literal constrains it.
            "DEFINE FIELD s ON t TYPE string DEFAULT 'anything at all';",
        ] {
            assert!(!has_2001(query), "codes for {query:?}: {:?}", codes(query));
        }
    }

    /// The invariant whose absence let the DEFINE-site gap survive a
    /// thousand-test suite: a value either inhabits a field's declared type or
    /// it does not, and *where* it appears cannot change the answer. Declaring
    /// it (`DEFAULT <v>`) and writing it (`SET f = <v>`) must reach the same
    /// verdict for every pairing — including the ones where both stay silent.
    #[test]
    fn declaring_a_value_and_writing_it_reach_the_same_verdict() {
        for (declared_type, value) in [
            // Scalar literal unions: the shape the fix is about.
            ("'red' | 'blue'", "'green'"),
            ("'red' | 'blue'", "'red'"),
            ("'red' | 'blue'", "42"),
            ("option<'red' | 'blue'>", "'green'"),
            ("option<'red' | 'blue'>", "'blue'"),
            ("1 | 2 | 3", "9"),
            ("1 | 2 | 3", "2"),
            // Not statically known — silent on both sides.
            ("'red' | 'blue'", "$colour"),
            ("'red' | 'blue'", "string::lowercase('RED')"),
            // A literal union nested in an object literal, which keeps its
            // element kinds through inference and so is caught on both sides.
            ("{ c: 'red' | 'blue' }", "{ c: 'green' }"),
            ("{ c: 'red' | 'blue' }", "{ c: 'red' }"),
            // A literal union nested in an array: inference widens the element
            // to `string`, so BOTH sides stay silent. Symmetric, and therefore
            // a precision limit rather than this bug.
            ("array<'red' | 'blue'>", "['green']"),
            ("array<'red' | 'blue'>", "['red']"),
            // Plain kinds, where no literal narrowing is in play at all.
            ("string", "'anything at all'"),
            ("int", "'nope'"),
            ("int", "7"),
        ] {
            let at_definition = has_2001(&format!(
                "DEFINE FIELD f ON t TYPE {declared_type} DEFAULT {value};"
            ));
            let at_write = has_2001(&format!(
                "DEFINE FIELD f ON t TYPE {declared_type};\nCREATE t SET f = {value};"
            ));
            assert_eq!(
                at_definition, at_write,
                "`{declared_type}` vs `{value}`: DEFINE site says {at_definition}, write site says {at_write}"
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
            (
                "option",
                option_of(user_link()),
                vec![KindWrapper::Optional],
            ),
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
        // idiom flattening implies — and distributes, so the answer is still a
        // collection (`items.sku` on an `array<{ sku }>` is an `array<string>`).
        assert_eq!(
            subkind_at(&kind, &[named("sku")]),
            Some(Kind::Array(Box::new(Kind::String), None))
        );
        assert!(subkind_at(&kind, &[named("ghost")]).is_none());
        // An open object proves nothing about its members, so it never claims
        // one exists.
        assert!(subkind_at(&Kind::Object, &[named("anything")]).is_none());
    }

    // -----------------------------------------------------------------------
    // project: one rule per policy bullet
    // -----------------------------------------------------------------------

    fn field(name: &str) -> crate::schema::FieldStep {
        crate::schema::FieldStep::Field(name.to_string())
    }

    fn schema_of(text: &str) -> crate::schema::SchemaIndex {
        let parsed = surrealql_analyzer_syntax::parse::parse_source(
            surrealql_analyzer_syntax::source::SourceId::new("schema:project"),
            text,
        )
        .expect("schema parses");
        crate::schema::extract_schema(&[parsed]).schema
    }

    #[test]
    fn project_any_is_any() {
        use crate::schema::FieldStep::Element;
        assert_eq!(project(&Kind::Any, &field("x"), None), Some(Kind::Any));
        assert_eq!(project(&Kind::Any, &Element, None), Some(Kind::Any));
    }

    #[test]
    fn project_reads_a_literal_object_by_key_and_nothing_else() {
        use crate::schema::FieldStep::Element;
        let obj = object(&[("name", Kind::String)]);
        assert_eq!(project(&obj, &field("name"), None), Some(Kind::String));
        assert_eq!(project(&obj, &field("ghost"), None), None);
        assert_eq!(project(&obj, &Element, None), None);
        // An open object, a scalar, a table and a tuple prove no member.
        assert_eq!(project(&Kind::Object, &field("name"), None), None);
        assert_eq!(project(&Kind::String, &field("len"), None), None);
        assert_eq!(
            project(&Kind::Table(vec![Table::from("t")]), &field("id"), None),
            None
        );
        assert_eq!(
            project(
                &Kind::Literal(KindLiteral::Array(vec![Kind::Int])),
                &Element,
                None
            ),
            None
        );
    }

    #[test]
    fn project_keeps_the_option_around_a_member() {
        let obj = object(&[("name", Kind::String)]);
        // `none | {…}` and `null | {…}` both come back as `none | member`.
        assert_eq!(
            project(&option_of(obj.clone()), &field("name"), None),
            Some(option_of(Kind::String))
        );
        assert_eq!(
            project(
                &Kind::either(vec![Kind::Null, obj.clone()]),
                &field("name"),
                None
            ),
            Some(option_of(Kind::String))
        );
        // A member that is already optional does not double-wrap.
        let nested = object(&[("nick", option_of(Kind::String))]);
        assert_eq!(
            project(&option_of(nested), &field("nick"), None),
            Some(option_of(Kind::String))
        );
        // No payload arm has the member: nothing, not `option<nothing>`.
        assert_eq!(project(&option_of(obj), &field("ghost"), None), None);
        assert_eq!(
            project(
                &Kind::either(vec![Kind::None, Kind::Null]),
                &field("x"),
                None
            ),
            None
        );
    }

    #[test]
    fn project_distributes_over_a_union_and_drops_arms_without_the_member() {
        let a = object(&[("x", Kind::Int), ("only_a", Kind::Bool)]);
        let b = object(&[("x", Kind::String)]);
        let union = Kind::either(vec![a, b]);
        // Both arms have `x`: the union of their answers, in arm order.
        assert_eq!(
            project(&union, &field("x"), None),
            Some(Kind::either(vec![Kind::Int, Kind::String]))
        );
        // One arm has it: that arm's answer alone.
        assert_eq!(project(&union, &field("only_a"), None), Some(Kind::Bool));
        // Neither: nothing.
        assert_eq!(project(&union, &field("ghost"), None), None);
        // Optional multi-arm union: option of the union.
        assert_eq!(
            project(&Kind::either(vec![Kind::None, union]), &field("x"), None),
            Some(Kind::either(vec![Kind::None, Kind::Int, Kind::String]))
        );
    }

    #[test]
    fn project_distributes_over_collections_and_wraps_back() {
        use crate::schema::FieldStep::Element;
        let element = object(&[("sku", Kind::String)]);
        let array = Kind::Array(Box::new(element.clone()), Some(3));
        let set = Kind::Set(Box::new(element.clone()), None);
        // `Element` is the element itself.
        assert_eq!(project(&array, &Element, None), Some(element.clone()));
        assert_eq!(project(&set, &Element, None), Some(element));
        // A field step distributes and keeps the collection and its length.
        assert_eq!(
            project(&array, &field("sku"), None),
            Some(Kind::Array(Box::new(Kind::String), Some(3)))
        );
        assert_eq!(
            project(&set, &field("sku"), None),
            Some(Kind::Set(Box::new(Kind::String), None))
        );
        assert_eq!(project(&array, &field("ghost"), None), None);
        // `option<array<T>>[*]` is `option<T>`.
        assert_eq!(
            project(
                &option_of(Kind::Array(Box::new(Kind::Int), None)),
                &Element,
                None
            ),
            Some(option_of(Kind::Int))
        );
    }

    #[test]
    fn project_reads_record_links_through_the_schema() {
        let schema = schema_of(
            "DEFINE TABLE user SCHEMAFULL;\n\
             DEFINE FIELD name ON user TYPE string;\n\
             DEFINE FIELD age ON user TYPE int;\n\
             DEFINE TABLE bot SCHEMAFULL;\n\
             DEFINE FIELD name ON bot TYPE option<string>;\n\
             DEFINE FIELD model ON bot TYPE string;",
        );
        let user = Kind::Record(vec![Table::from("user")]);
        let both = Kind::Record(vec![Table::from("user"), Table::from("bot")]);

        // Single table: its field, including the implicit `id`.
        assert_eq!(
            project(&user, &field("name"), Some(&schema)),
            Some(Kind::String)
        );
        assert_eq!(
            project(&user, &field("id"), Some(&schema)),
            Some(user.clone())
        );
        assert_eq!(project(&user, &field("ghost"), Some(&schema)), None);
        // Several tables: the union of what every arm answers — and an arm
        // that does not declare the field answers `none` (3.2.3:
        // `type::of(owner.username)` over `record<account | organization>` is
        // `['string', 'none']`), so `model` is `none | string`, not `string`.
        assert_eq!(
            project(&both, &field("name"), Some(&schema)),
            Some(Kind::either(vec![Kind::String, option_of(Kind::String)]))
        );
        assert_eq!(
            project(&both, &field("model"), Some(&schema)),
            Some(Kind::either(vec![Kind::None, Kind::String]))
        );
        assert_eq!(project(&both, &field("ghost"), Some(&schema)), None);
        // Without a schema, through an unknown table, or on `record<>`: unprovable.
        assert_eq!(project(&user, &field("name"), None), None);
        assert_eq!(
            project(
                &Kind::Record(vec![Table::from("nope")]),
                &field("name"),
                Some(&schema)
            ),
            None
        );
        assert_eq!(
            project(&Kind::Record(vec![]), &field("id"), Some(&schema)),
            None
        );
        assert_eq!(
            project(&user, &crate::schema::FieldStep::Element, Some(&schema)),
            None
        );
        // Wrappers compose with the link: `option<array<record<user>>>.name`.
        let links = option_of(Kind::Array(Box::new(user), None));
        assert_eq!(
            project(&links, &field("name"), Some(&schema)),
            Some(option_of(Kind::Array(Box::new(Kind::String), None)))
        );
    }

    #[test]
    fn project_path_walks_step_by_step_and_stops_at_the_first_missing_member() {
        let schema = schema_of(
            "DEFINE TABLE user SCHEMAFULL;\n\
             DEFINE FIELD name ON user TYPE string;\n\
             DEFINE TABLE post SCHEMAFULL;\n\
             DEFINE FIELD author ON post TYPE option<record<user>>;",
        );
        let post = Kind::Record(vec![Table::from("post")]);
        assert_eq!(project_path(&post, &[], Some(&schema)), Some(post.clone()));
        assert_eq!(
            project_fields(&post, &["author".into(), "name".into()], Some(&schema)),
            Some(option_of(Kind::String))
        );
        assert_eq!(
            project_fields(&post, &["author".into(), "ghost".into()], Some(&schema)),
            None
        );
        assert_eq!(
            project_fields(&post, &["ghost".into(), "name".into()], Some(&schema)),
            None
        );
    }
}
