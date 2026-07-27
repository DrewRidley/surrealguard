//! The `Kind` lattice: `meet` (greatest lower bound) and `subtract`.
//!
//! Refinement today is a table of ad-hoc transforms — `strip_variant` removes
//! `Kind::None`, `narrow_out_none` removes `None` *and* `Null`, `eq_narrow`
//! compares two base kinds for `==`, `narrow_record_to` rewrites a table set —
//! each written for one recognizer and each with its own soundness edge. They
//! disagree: `narrow.rs`'s `StripNone` and `infer.rs`'s `NotNone` are the same
//! fact with two different answers, and one of them drops a `null` that
//! survives the guard at runtime.
//!
//! All of them are instances of two order-theoretic operations, which is what
//! this module provides:
//!
//! * [`meet`] — the greatest kind below both operands. A positive refinement
//!   (`p = 'active'`, `type::table(p) = 'user'`, `p IS NOT NONE` read as "p is
//!   in the non-`NONE` part of its kind") is a meet with the fact's
//!   characteristic kind.
//! * [`subtract`] — `a` minus everything `b` covers. A negative refinement
//!   (`p != NONE`, `type::table(p) != 'user'`) is a subtraction.
//!
//! ## The order
//!
//! `a ⊑ b` is [`crate::kinds::kind_is_assignable_to(a, b)`]. Reusing it rather
//! than inventing a second notion is deliberate: two relations would drift
//! exactly as the transforms above did. It is a *pre*order rather than a
//! partial order — see "The literal asymmetry" below — and this module is
//! written to be well-defined anyway.
//!
//! ## The safety direction — the invariant that matters
//!
//! Every operation here has to satisfy two things at once:
//!
//! * **precision**: `meet(a, b) ⊑ a` and `meet(a, b) ⊑ b`, and
//!   `subtract(a, b) ⊑ a` — a refinement never widens past its input;
//! * **soundness**: the result must admit every value that really does satisfy
//!   the refinement — it may be *wider* than the truth, never narrower.
//!
//! For [`subtract`] the two coexist easily: when the exact difference is not
//! representable, returning `a` itself is both `⊑ a` and wider than the truth.
//!
//! For [`meet`] they do not. `result ⊑ a` and `result ⊑ b` already force
//! `result ⊆ a ∩ b`; adding "never narrower than the truth" forces
//! `result = a ∩ b` **exactly**. So a meet that is not exactly representable
//! cannot be reported as a kind at all without breaking one of the two, and
//! [`KindMeet::Unrepresentable`] is the honest answer: the caller keeps its own
//! (wider) input. That is what makes prove-or-stay-silent hold once consumers
//! arrive — a refinement that cannot be computed leaves the type alone instead
//! of guessing a narrower one.
//!
//! [`KindMeet::Empty`] is the opposite claim and is only ever returned when
//! disjointness is *proven*: it is what a future `decide` reads as
//! "this branch is dead".
//!
//! ## The literal asymmetry
//!
//! `kind_is_assignable_to` deliberately accepts `string` into a `'active'`
//! target (see its doc comment: inference widens a written `'active'` to
//! `string`, and a `string` can never be *proven* to fall outside a
//! string-literal union). That makes `string ⊑ 'active'` *and*
//! `'active' ⊑ string` both true, so the relation is not antisymmetric, and a
//! naive "if `a ⊑ b` then `a` is the meet" would return whichever operand came
//! first — a non-commutative `meet`.
//!
//! Two rules keep this module well-defined:
//!
//! * scalar literals are handled *before* the subtyping shortcut ([`meet`]
//!   step 4), so `meet(string, 'active') = 'active'` — the singleton, which is
//!   the answer §1.3 of the expression-fact design needs;
//! * the shortcut only fires when the relation is *strictly* one-directional.
//!   A mutually-assignable pair falls through to the structural rules, which
//!   are symmetric by construction.
//!
//! ## No consumers
//!
//! Nothing calls these yet, on purpose: this module must not change a single
//! inferred type or diagnostic. The property tests below are the deliverable.

#![allow(dead_code)]

use std::collections::BTreeMap;

use surrealdb_types::{Kind, KindLiteral};

use crate::kinds::{
    kind_admits_none, kind_is_assignable_to, literal_base_kind, scalar_literal_base_kind,
};

/// The result of [`meet`].
///
/// Three outcomes rather than an `Option<Kind>`, because "provably nothing" and
/// "cannot be expressed" are opposite claims and a consumer must not confuse
/// them: the first says a branch is dead, the second says stay silent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum KindMeet {
    /// Proven disjoint: no value inhabits both operands.
    Empty,
    /// The exact greatest lower bound. Guaranteed `⊑` both operands.
    Exact(Kind),
    /// The greatest lower bound exists but cannot be written as a `Kind`.
    /// Callers keep their own input unchanged — see the module docs.
    Unrepresentable,
}

impl KindMeet {
    /// The refined kind, or `fallback` when the meet proved nothing usable.
    /// This is how a refinement consumer will read a meet: narrow if we can,
    /// keep the input if we cannot.
    pub(crate) fn or_keep(self, fallback: &Kind) -> Kind {
        match self {
            KindMeet::Exact(kind) => kind,
            KindMeet::Empty | KindMeet::Unrepresentable => fallback.clone(),
        }
    }

    /// Chains a third operand onto a meet, so `meet(meet(a, b), c)` is
    /// expressible. `Empty` absorbs; `Unrepresentable` propagates.
    pub(crate) fn meet_with(self, other: &Kind) -> KindMeet {
        match self {
            KindMeet::Empty => KindMeet::Empty,
            KindMeet::Exact(kind) => meet(&kind, other),
            KindMeet::Unrepresentable => KindMeet::Unrepresentable,
        }
    }
}

/// The greatest kind below both `a` and `b`.
///
/// `meet(x, any) == x` (`any` is top), `meet(x, x) == x`, and the operation is
/// commutative and associative wherever it is representable. Every
/// [`KindMeet::Exact`] result is `⊑` both operands; [`KindMeet::Empty`] is only
/// returned when disjointness is proven; anything else is
/// [`KindMeet::Unrepresentable`] and the caller must keep its input.
///
/// Over-approximations returned deliberately, and why:
///
/// * **`int` against `float`/`decimal`** → `int`. `kind_is_assignable_to`
///   admits `int` into both as a *coercion*, not as value inclusion, so under
///   that order `int` really is below both even though no value is at once an
///   int and a float. Returning `int` is the wider answer.
/// * **`float` against `decimal`** → `Unrepresentable`, for the same reason
///   read the other way: the two share the lower bound `int`, so they are not
///   disjoint under this order, and their exact meet has no name.
/// * **two collections with disjoint elements** (`array<int>` ∧
///   `array<string>`) → `array<any, 0>`. Only the empty collection inhabits
///   both, and a zero-length collection says exactly that. Any zero-length
///   result is canonicalised to an `any` element: the element kind of an empty
///   collection is unobservable, and leaving it free would break associativity.
/// * **two closed object literals** → the meet of their shared properties, but
///   only if every property one side declares and the other omits admits
///   `NONE`. `object_is_assignable_to` reads an object literal as *closed* (no
///   extra properties, omitted target properties must be optional), so a
///   property required by one side and unknown to the other is a proof of
///   disjointness, not something to merge.
/// * **two array literals of equal arity** → `Unrepresentable`; of differing
///   arity → `Empty`. The order has no rule relating two tuples, so a
///   position-wise meet could not be shown to be below either operand.
/// * **a non-scalar literal (`[a, b]`, `{ a: … }`) against a non-literal** →
///   `Empty` when their base kinds are disjoint, `Unrepresentable` otherwise.
///   The base reduction throws away the structure the exact answer needs.
/// * **anything else of the same `Kind` variant that no rule above covers**
///   (`geometry<point>` vs `geometry<line>`, `file<a>` vs `file<b>`) →
///   `Unrepresentable`. Their arguments are sets that may overlap, and this
///   module does not model them; claiming `Empty` would be a false proof of a
///   dead branch.
///
/// Two operands of *different* `Kind` variants that reach the end are disjoint:
/// a value is a string or an int or a record, never two of them. The coercions
/// and reductions that cross variants (numerics, literals, `any`, unions) are
/// all handled before that point.
pub(crate) fn meet(a: &Kind, b: &Kind) -> KindMeet {
    // 1. `any` is top.
    if matches!(a, Kind::Any) {
        return KindMeet::Exact(b.clone());
    }
    if matches!(b, Kind::Any) {
        return KindMeet::Exact(a.clone());
    }

    // 2. Idempotence, and the fast path for every equal pair.
    if a == b {
        return KindMeet::Exact(a.clone());
    }

    // 3. A union is a set of alternatives, so the meet distributes over it.
    //    When BOTH sides are unions the choice of which one to walk decides the
    //    variant ORDER of the result. A `Kind` is compared structurally — by
    //    `==`, which is sequence-sensitive — everywhere a consumer stores or
    //    reports one, so two permutations of the same set are two different
    //    answers and `meet` would not be commutative. Picking the operand by a
    //    total order on the rendered kind makes `meet(a, b)` and `meet(b, a)`
    //    walk the same one.
    match (a, b) {
        (Kind::Either(left), Kind::Either(right)) => {
            return if a.to_string() <= b.to_string() {
                meet_over_union(left, b)
            } else {
                meet_over_union(right, a)
            };
        }
        (Kind::Either(variants), _) => return meet_over_union(variants, b),
        (_, Kind::Either(variants)) => return meet_over_union(variants, a),
        _ => {}
    }

    // 4. A scalar literal is a singleton, and singletons decide the meet
    //    outright: either the value inhabits the other kind (so the literal is
    //    the meet) or it does not (so they are disjoint). This runs BEFORE the
    //    subtyping shortcut because `kind_is_assignable_to` deliberately admits
    //    a bare `string` into a `'active'` target — see the module docs.
    match (scalar_literal_base_kind(a), scalar_literal_base_kind(b)) {
        // Two different single values.
        (Some(_), Some(_)) => return KindMeet::Empty,
        (Some(base), None) => return singleton_meet(a, &base, b),
        (None, Some(base)) => return singleton_meet(b, &base, a),
        (None, None) => {}
    }

    // 5. When one side is strictly below the other, it *is* the meet. Strictly:
    //    a mutually-assignable pair (which the literal rule above can still
    //    produce nested inside an object or a collection) falls through to the
    //    structural rules, which are symmetric.
    match (kind_is_assignable_to(a, b), kind_is_assignable_to(b, a)) {
        (true, false) => return KindMeet::Exact(a.clone()),
        (false, true) => return KindMeet::Exact(b.clone()),
        (true, true) | (false, false) => {}
    }

    meet_structural(a, b)
}

/// `a` minus every value `b` admits.
///
/// `None` is bottom: `b` covers all of `a`, so the refinement leaves nothing
/// and a consumer reading this may conclude the branch is dead. `Some(k)`
/// satisfies `k ⊑ a` and admits every value of `a` that `b` does *not* — never
/// fewer.
///
/// Exact where the difference has a name, and deliberately wide where it does
/// not:
///
/// * `subtract(option<string>, none) = string` — a union drops the variants
///   that are covered, which is what every `StripNone`/`NotNone` transform in
///   `flow/narrow.rs` and `expression/infer.rs` is really doing. Because the
///   variants are subtracted one at a time, `none` and `null` cannot be
///   confused: subtracting `none` from `none | null | string` keeps the
///   `null`, which is the runtime truth the two existing transforms disagree
///   about.
/// * `subtract(record<user | folder>, record<folder>) = record<user>` — table
///   sets subtract.
/// * `subtract(string, 'a') = string` — removing one value from an infinite
///   kind has no name, so the input is returned unchanged. This is the wider
///   answer, and the reason a `!=` against a non-literal kind must stay a
///   no-op rather than become a "not quite string".
/// * `subtract(any, string) = any` — same reason, at the top of the lattice.
/// * `subtract(number, int) = number` — the numeric tower is a coercion order
///   (see [`meet`]), so the difference is not `float | decimal`.
pub(crate) fn subtract(a: &Kind, b: &Kind) -> Option<Kind> {
    // Removing a kind from itself leaves nothing, whatever the kind is —
    // including `any`, whose difference is otherwise never representable.
    if a == b {
        return None;
    }

    // A union subtracts variant-wise. This is the case that keeps the two
    // sentinels apart, so it must come before any whole-kind reasoning.
    if let Kind::Either(variants) = a {
        let kept: Vec<Kind> = variants
            .iter()
            .filter_map(|variant| subtract(variant, b))
            .collect();
        return (!kept.is_empty()).then(|| canonical_union(kept));
    }

    match meet(a, b) {
        // Disjoint: `b` removes nothing.
        KindMeet::Empty => Some(a.clone()),
        // `b` covers all of `a`.
        KindMeet::Exact(common) if common == *a => None,
        // A partial overlap, or an overlap that could not be computed. Either
        // way the difference is only representable for a record's table set;
        // everywhere else the input is the sound answer.
        KindMeet::Exact(_) | KindMeet::Unrepresentable => residual(a, b),
    }
}

/// Whether `a` and `b` are *proven* to share no value. Only ever true on a
/// proof, so a `false` means "not known to be disjoint", not "they overlap".
pub(crate) fn kinds_are_disjoint(a: &Kind, b: &Kind) -> bool {
    matches!(meet(a, b), KindMeet::Empty)
}

// ---------------------------------------------------------------------------
// internals
// ---------------------------------------------------------------------------

/// `meet(Either(variants), other)`, distributed. A variant whose meet is empty
/// drops out; if any variant's meet is unrepresentable the union's is too,
/// because the part that could not be named cannot be left out of the answer.
fn meet_over_union(variants: &[Kind], other: &Kind) -> KindMeet {
    let mut kept = Vec::with_capacity(variants.len());
    for variant in variants {
        match meet(variant, other) {
            KindMeet::Empty => {}
            KindMeet::Exact(kind) => kept.push(kind),
            KindMeet::Unrepresentable => return KindMeet::Unrepresentable,
        }
    }
    if kept.is_empty() {
        return KindMeet::Empty;
    }
    KindMeet::Exact(canonical_union(kept))
}

/// The meet of a scalar literal (`literal`, whose base kind is `base`) with a
/// non-literal `other`: the literal itself when its base overlaps `other`,
/// nothing when it cannot.
fn singleton_meet(literal: &Kind, base: &Kind, other: &Kind) -> KindMeet {
    match meet(base, other) {
        KindMeet::Empty => KindMeet::Empty,
        KindMeet::Exact(_) => KindMeet::Exact(literal.clone()),
        KindMeet::Unrepresentable => KindMeet::Unrepresentable,
    }
}

/// The shape-directed half of [`meet`], reached only for two kinds neither of
/// which is `any`, a union, a scalar literal, or strictly below the other.
fn meet_structural(a: &Kind, b: &Kind) -> KindMeet {
    match (a, b) {
        // Table sets intersect. An EMPTY set is `record<>` — any record — and
        // is a supertype of every record kind, so the subtyping shortcut has
        // already resolved those; both sets are constrained here.
        (Kind::Record(left), Kind::Record(right)) => {
            let common: Vec<_> = left
                .iter()
                .filter(|table| right.contains(table))
                .cloned()
                .collect();
            if common.is_empty() {
                KindMeet::Empty
            } else {
                KindMeet::Exact(Kind::Record(common))
            }
        }
        (Kind::Array(left, left_len), Kind::Array(right, right_len)) => {
            meet_collection(left, *left_len, right, *right_len, Collection::Array)
        }
        (Kind::Set(left, left_len), Kind::Set(right, right_len)) => {
            meet_collection(left, *left_len, right, *right_len, Collection::Set)
        }
        (Kind::Literal(KindLiteral::Object(left)), Kind::Literal(KindLiteral::Object(right))) => {
            meet_objects(left, right)
        }
        (Kind::Literal(KindLiteral::Array(left)), Kind::Literal(KindLiteral::Array(right))) => {
            meet_tuples(left, right)
        }
        _ => {
            // The numeric tower is a coercion order, not value inclusion: `int`
            // is admitted into both `float` and `decimal`, so those two share a
            // lower bound and are NOT disjoint under this order — but their
            // exact meet has no name.
            if is_numeric(a) && is_numeric(b) {
                return KindMeet::Unrepresentable;
            }
            // An object/array literal against something structural: its base
            // kind can prove disjointness but cannot express the overlap,
            // because reducing it throws away the structure.
            if let Some(base) = literal_base_kind(a) {
                return degrade_to_base(&base, b);
            }
            if let Some(base) = literal_base_kind(b) {
                return degrade_to_base(&base, a);
            }
            // Same variant, no rule: the variant's arguments (a geometry set, a
            // file bucket, a reference target) may overlap and are not modelled
            // here. Claiming `Empty` would be a false proof of a dead branch.
            if std::mem::discriminant(a) == std::mem::discriminant(b) {
                return KindMeet::Unrepresentable;
            }
            // Different variants, and every cross-variant relation (`any`,
            // unions, literals, numerics) has been handled above: a value
            // cannot be both.
            KindMeet::Empty
        }
    }
}

/// A literal's base kind can only ever *disprove* an overlap.
fn degrade_to_base(base: &Kind, other: &Kind) -> KindMeet {
    match meet(base, other) {
        KindMeet::Empty => KindMeet::Empty,
        KindMeet::Exact(_) | KindMeet::Unrepresentable => KindMeet::Unrepresentable,
    }
}

#[derive(Clone, Copy)]
enum Collection {
    Array,
    Set,
}

impl Collection {
    fn build(self, element: Kind, len: Option<u64>) -> Kind {
        match self {
            Collection::Array => Kind::Array(Box::new(element), len),
            Collection::Set => Kind::Set(Box::new(element), len),
        }
    }
}

/// Collections meet element-wise, and their lengths meet at the smaller bound
/// (`length_fits` reads a declared length as an upper bound, so `array<T, 3>`
/// is below `array<T, 5>`).
///
/// When the elements are disjoint the only shared inhabitant is the *empty*
/// collection, which `array<any, 0>` expresses exactly. Every zero-length
/// result is canonicalised to an `any` element for the same reason: the element
/// kind of an empty collection is unobservable, and leaving it free would make
/// `meet` non-associative (`(array<int> ∧ array<string>) ∧ array<bool>` and
/// `array<int> ∧ (array<string> ∧ array<bool>)` would disagree on a kind no
/// value can ever exhibit).
fn meet_collection(
    left: &Kind,
    left_len: Option<u64>,
    right: &Kind,
    right_len: Option<u64>,
    collection: Collection,
) -> KindMeet {
    let len = match (left_len, right_len) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(only), None) | (None, Some(only)) => Some(only),
        (None, None) => None,
    };
    let element = match meet(left, right) {
        KindMeet::Empty => return KindMeet::Exact(collection.build(Kind::Any, Some(0))),
        KindMeet::Exact(kind) => kind,
        KindMeet::Unrepresentable => return KindMeet::Unrepresentable,
    };
    if len == Some(0) {
        return KindMeet::Exact(collection.build(Kind::Any, Some(0)));
    }
    KindMeet::Exact(collection.build(element, len))
}

/// Two closed object literals meet on the properties they share.
///
/// `object_is_assignable_to` reads an object literal as closed: a value of that
/// kind carries no property the literal does not declare, and may omit only the
/// ones that admit `NONE`. So a property one side requires and the other never
/// declares can be satisfied by no value of both kinds at once — that is a
/// proof of disjointness, not an invitation to union the property sets.
fn meet_objects(left: &BTreeMap<String, Kind>, right: &BTreeMap<String, Kind>) -> KindMeet {
    let one_sided_required = |from: &BTreeMap<String, Kind>, other: &BTreeMap<String, Kind>| {
        from.iter()
            .any(|(key, kind)| !other.contains_key(key) && !kind_admits_none(kind))
    };
    if one_sided_required(left, right) || one_sided_required(right, left) {
        return KindMeet::Empty;
    }

    let mut shared = BTreeMap::new();
    for (key, left_kind) in left {
        let Some(right_kind) = right.get(key) else {
            continue;
        };
        match meet(left_kind, right_kind) {
            // No value satisfies both declarations of a shared property, and
            // the property cannot be omitted (an omittable one would have kept
            // `NONE` in the meet), so nothing inhabits both objects.
            KindMeet::Empty => return KindMeet::Empty,
            KindMeet::Exact(kind) => {
                shared.insert(key.clone(), kind);
            }
            KindMeet::Unrepresentable => return KindMeet::Unrepresentable,
        }
    }
    KindMeet::Exact(Kind::Literal(KindLiteral::Object(shared)))
}

/// Two array literals are fixed-arity tuples. Differing arities are a proof of
/// disjointness; equal arities are not computed.
///
/// The position-wise meet is easy to write and is deliberately NOT written:
/// `kind_is_assignable_to` has no rule relating two array literals at all
/// (`[int, 'a']` is not assignable to `[int, string]`), so a position-wise
/// result could not be shown to be below either operand — it would satisfy the
/// lattice's soundness core only by the reader's say-so. `Unrepresentable`
/// keeps the caller's own kind, which is the wider answer, until the order
/// itself learns about tuples.
fn meet_tuples(left: &[Kind], right: &[Kind]) -> KindMeet {
    if left.len() == right.len() {
        KindMeet::Unrepresentable
    } else {
        KindMeet::Empty
    }
}

/// What is left of `a` once `b` has taken its overlap, for the shapes whose
/// difference has a name. Everything else keeps `a`.
fn residual(a: &Kind, b: &Kind) -> Option<Kind> {
    if let (Kind::Record(left), Kind::Record(right)) = (a, b) {
        // An empty set on either side is `record<>` — any record. As a source
        // there is nothing to enumerate away; as a target it covers everything,
        // which `subtract`'s coverage test has already handled.
        if !left.is_empty() && !right.is_empty() {
            let kept: Vec<_> = left
                .iter()
                .filter(|table| !right.contains(table))
                .cloned()
                .collect();
            return (!kept.is_empty()).then_some(Kind::Record(kept));
        }
    }
    Some(a.clone())
}

fn is_numeric(kind: &Kind) -> bool {
    matches!(kind, Kind::Int | Kind::Float | Kind::Decimal | Kind::Number)
}

/// A union built from the variants that survived, in the order they were
/// walked.
///
/// The order is load-bearing and must NOT be normalised away. Assignability no
/// longer cares about it — a union source is checked variant by variant, so
/// `'c' | 'a' | 'b'` and `'a' | 'b' | 'c'` are mutually assignable — but a
/// `Kind` is still *equal* only to the same sequence, and that is what a
/// consumer stores, renders and compares. Re-ordering would make
/// `meet(k, any-supertype) == k` fail on a kind that only permutes `k`.
/// Walking a sub-sequence of one operand's variants keeps the result identical
/// to that operand where nothing was removed; [`meet`] step 3 is what makes the
/// choice of operand deterministic.
///
/// `Kind::either` flattens, de-duplicates and collapses a singleton. It maps an
/// EMPTY list to `Kind::None`, which would be a silent lie here, so every
/// caller checks for empty first and reports [`KindMeet::Empty`] instead.
fn canonical_union(kinds: Vec<Kind>) -> Kind {
    debug_assert!(!kinds.is_empty(), "an empty union is bottom, not `none`");
    Kind::either(kinds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealdb_types::Table;

    // -----------------------------------------------------------------------
    // the generator
    // -----------------------------------------------------------------------

    fn string_literal(value: &str) -> Kind {
        Kind::Literal(KindLiteral::String(value.to_string()))
    }

    fn int_literal(value: i64) -> Kind {
        Kind::Literal(KindLiteral::Integer(value))
    }

    fn object(pairs: &[(&str, Kind)]) -> Kind {
        Kind::Literal(KindLiteral::Object(
            pairs
                .iter()
                .map(|(key, kind)| ((*key).to_string(), kind.clone()))
                .collect(),
        ))
    }

    fn record(tables: &[&str]) -> Kind {
        Kind::Record(tables.iter().map(|name| Table::from(*name)).collect())
    }

    fn option_of(inner: Kind) -> Kind {
        Kind::either(vec![Kind::None, inner])
    }

    /// A hand-rolled, exhaustive universe rather than a `proptest` dependency.
    ///
    /// The kind space that matters is small and enumerable: the shapes below
    /// are every one `meet`/`subtract` has a rule for, plus the ones they must
    /// decline. A generator over ~50 kinds gives 2 500 pairs and 125 000
    /// triples — checked in full, in milliseconds, with no shrinking machinery
    /// and no randomness to reproduce. A property that fails here fails with a
    /// named counterexample, every run.
    fn universe() -> Vec<Kind> {
        vec![
            // top, and the two sentinels that are their own kinds
            Kind::Any,
            Kind::None,
            Kind::Null,
            // scalars, including the numeric tower
            Kind::Bool,
            Kind::String,
            Kind::Int,
            Kind::Float,
            Kind::Decimal,
            Kind::Number,
            Kind::Datetime,
            Kind::Duration,
            Kind::Uuid,
            Kind::Bytes,
            Kind::Object,
            // scalar literals and their unions
            string_literal("active"),
            string_literal("inactive"),
            int_literal(1),
            int_literal(2),
            Kind::Literal(KindLiteral::Bool(true)),
            Kind::either(vec![string_literal("active"), string_literal("inactive")]),
            Kind::either(vec![
                string_literal("active"),
                string_literal("inactive"),
                string_literal("banned"),
            ]),
            Kind::either(vec![int_literal(1), int_literal(2)]),
            // options, and the option-of-nullable that keeps the two sentinels
            // apart
            option_of(Kind::String),
            option_of(Kind::Int),
            Kind::either(vec![Kind::None, Kind::Null, Kind::String]),
            Kind::either(vec![Kind::Null, Kind::String]),
            option_of(string_literal("active")),
            option_of(Kind::either(vec![
                string_literal("active"),
                string_literal("inactive"),
            ])),
            // unrelated unions
            Kind::either(vec![Kind::Int, Kind::String]),
            Kind::either(vec![Kind::Bool, Kind::Datetime]),
            // records, one and several tables, plus the unconstrained one
            record(&["user"]),
            record(&["folder"]),
            record(&["user", "folder"]),
            record(&["folder", "post"]),
            record(&[]),
            option_of(record(&["user", "folder"])),
            // collections, with and without a length, array and set
            Kind::Array(Box::new(Kind::String), None),
            Kind::Array(Box::new(Kind::String), Some(3)),
            Kind::Array(Box::new(Kind::String), Some(5)),
            Kind::Array(Box::new(Kind::Int), None),
            Kind::Array(Box::new(Kind::Number), None),
            Kind::Array(Box::new(Kind::Any), Some(0)),
            Kind::Set(Box::new(Kind::String), None),
            Kind::Set(Box::new(Kind::String), Some(3)),
            Kind::Array(Box::new(option_of(Kind::String)), None),
            option_of(Kind::Array(Box::new(Kind::String), None)),
            Kind::Array(Box::new(record(&["user", "folder"])), None),
            // object literals: nested, overlapping, disjoint, and one whose
            // extra property is optional
            object(&[("name", Kind::String)]),
            object(&[("name", Kind::String), ("age", Kind::Int)]),
            object(&[("name", Kind::String), ("nick", option_of(Kind::String))]),
            object(&[("name", string_literal("active"))]),
            object(&[("age", Kind::Int)]),
            object(&[("inner", object(&[("flag", Kind::Bool)]))]),
            // array literals (fixed-arity tuples)
            Kind::Literal(KindLiteral::Array(vec![Kind::Int, Kind::String])),
            Kind::Literal(KindLiteral::Array(vec![
                Kind::Int,
                string_literal("active"),
            ])),
            Kind::Literal(KindLiteral::Array(vec![Kind::Int])),
        ]
    }

    /// `a ⊑ b`, the order this module is built on — a name for
    /// `kind_is_assignable_to` so the properties below read as order theory.
    ///
    /// This used to patch a gap in the relation: it tested an `Either` *target*
    /// before an `Either` *source* and returned early from the first branch, so
    /// a sub-union was not assignable to its own superset (`'b' | 'c'` into
    /// `'a' | 'b' | 'c'` asked only whether the whole left union fitted inside
    /// ONE right variant). Every refinement of a union yields a sub-union, so
    /// the lattice could not have narrowed a literal-union field at all under
    /// that reading. The relation itself now applies the source rule first and
    /// at every depth, and the patch is gone.
    fn below(sub: &Kind, sup: &Kind) -> bool {
        kind_is_assignable_to(sub, sup)
    }

    /// `sub`'s values all inhabit `sup` — `below` minus its one deliberate
    /// non-subtyping allowance.
    ///
    /// `kind_is_assignable_to` accepts a bare `string` into a `'active'` target
    /// because a `string` cannot be *proven* to fall outside it (that is what
    /// keeps a valid write to a literal-union field from being a false 2001).
    /// It is emphatically not a claim that every string is `'active'`, and the
    /// two properties below need value inclusion: they hunt for a kind that
    /// really does live inside two others, and the widening allowance would
    /// hand them `string` as a witness for every pair of unrelated string
    /// literals.
    ///
    /// Only that one rule is tightened; everything else defers to `below`.
    fn inhabits(sub: &Kind, sup: &Kind) -> bool {
        match sup {
            Kind::Either(variants) => variants.iter().any(|variant| inhabits(sub, variant)),
            _ if scalar_literal_base_kind(sup).is_some() => sub == sup,
            _ => below(sub, sup),
        }
    }

    // -----------------------------------------------------------------------
    // meet: the algebra
    // -----------------------------------------------------------------------

    /// `meet(a, b) == meet(b, a)`, for every pair. Without this the answer
    /// would depend on which operand a consumer happened to write first — and
    /// the mutually-assignable literal pairs (`string` / `'active'`) are
    /// exactly where a naive implementation stops being symmetric.
    #[test]
    fn meet_is_commutative() {
        for a in &universe() {
            for b in &universe() {
                assert_eq!(
                    meet(a, b),
                    meet(b, a),
                    "meet is not commutative for {a} and {b}"
                );
            }
        }
    }

    /// `meet(a, a) == a`. A refinement by a fact the kind already states must
    /// be a no-op, not a rewrite.
    #[test]
    fn meet_is_idempotent() {
        for a in &universe() {
            assert_eq!(meet(a, a), KindMeet::Exact(a.clone()), "meet({a}, {a})");
        }
    }

    /// `meet(meet(a, b), c) == meet(a, meet(b, c))`, wherever both groupings
    /// are representable. This is what lets a conjunction of facts be folded in
    /// any order — which the guard interpreter will do, because `Facts` is a map
    /// keyed by place and the conjuncts arrive in source order.
    ///
    /// Triples where either grouping is `Unrepresentable` are excluded on
    /// purpose: `Unrepresentable` is an absence of information, and information
    /// that was never there cannot be expected to reappear on the other side of
    /// the parentheses.
    #[test]
    fn meet_is_associative_where_representable() {
        let universe = universe();
        for a in &universe {
            for b in &universe {
                for c in &universe {
                    let left = meet(a, b).meet_with(c);
                    let right = meet(b, c).meet_with(a);
                    if left == KindMeet::Unrepresentable || right == KindMeet::Unrepresentable {
                        continue;
                    }
                    assert_eq!(left, right, "meet is not associative for {a}, {b}, {c}");
                }
            }
        }
    }

    /// `meet(a, any) == a` and `meet(any, a) == a` — `any` is the top of the
    /// lattice, so meeting with it proves nothing and must change nothing.
    #[test]
    fn any_is_the_identity_of_meet() {
        for a in &universe() {
            assert_eq!(
                meet(a, &Kind::Any),
                KindMeet::Exact(a.clone()),
                "meet({a}, any)"
            );
            assert_eq!(
                meet(&Kind::Any, a),
                KindMeet::Exact(a.clone()),
                "meet(any, {a})"
            );
        }
    }

    // -----------------------------------------------------------------------
    // meet: soundness
    // -----------------------------------------------------------------------

    /// **The soundness core.** Every kind `meet` reports is below BOTH inputs.
    /// A result that is not below `a` is a refinement that *widened* the type —
    /// it would let a consumer claim a value the input never admitted. A result
    /// that is not below `b` is a refinement that ignored the fact it was given.
    #[test]
    fn a_meet_is_below_both_operands() {
        for a in &universe() {
            for b in &universe() {
                let KindMeet::Exact(common) = meet(a, b) else {
                    continue;
                };
                assert!(
                    below(&common, a),
                    "meet({a}, {b}) = {common} is not below {a}"
                );
                assert!(
                    below(&common, b),
                    "meet({a}, {b}) = {common} is not below {b}"
                );
            }
        }
    }

    /// **The safety direction, stated as a test.** Where the exact answer is
    /// unavailable, `meet` must give up rather than guess — and giving up means
    /// `Unrepresentable`, which callers read as "keep the input", i.e. the
    /// WIDER answer. The one thing it may never do is report `Empty` for two
    /// kinds that share a value, because `Empty` is a proof of a dead branch.
    ///
    /// The witnesses below are the shapes whose meet this module declines to
    /// compute; each must come back `Unrepresentable`, never a narrowed kind
    /// and never `Empty`.
    #[test]
    fn an_uncomputable_meet_is_wider_never_narrower() {
        let witnesses: &[(&str, Kind, Kind)] = &[
            (
                "float and decimal share the lower bound `int` under the coercion order",
                Kind::Float,
                Kind::Decimal,
            ),
            (
                "two geometry sets may overlap and are not modelled",
                Kind::Geometry(vec![surrealdb_types::GeometryKind::Point]),
                Kind::Geometry(vec![surrealdb_types::GeometryKind::Line]),
            ),
            (
                "an array literal against a plain array loses its positions",
                Kind::Literal(KindLiteral::Array(vec![Kind::Int, Kind::String])),
                Kind::Array(Box::new(Kind::Int), None),
            ),
        ];
        for (why, a, b) in witnesses {
            assert_eq!(
                meet(a, b),
                KindMeet::Unrepresentable,
                "meet({a}, {b}) must decline: {why}"
            );
            // …and declining must leave the caller with its own kind.
            assert_eq!(meet(a, b).or_keep(a), a.clone());
        }
    }

    /// `Empty` is a proof, so it may only be returned for kinds that really do
    /// share nothing. The check: no kind in the universe is below both operands
    /// of a pair `meet` called disjoint. (`meet(a, b) = Empty` with some `c`
    /// below both would mean the branch `meet` just declared dead is reachable
    /// by every value of `c`.)
    #[test]
    fn an_empty_meet_is_never_claimed_over_a_shared_lower_bound() {
        let universe = universe();
        for a in &universe {
            for b in &universe {
                if meet(a, b) != KindMeet::Empty {
                    continue;
                }
                for c in &universe {
                    // `any` is above everything and `assignable` reads it as a
                    // wildcard TARGET, so it is below nothing but itself; the
                    // interesting witnesses are the concrete kinds.
                    assert!(
                        !(inhabits(c, a) && inhabits(c, b)),
                        "meet({a}, {b}) claimed Empty, but {c} inhabits both"
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // subtract
    // -----------------------------------------------------------------------

    /// `subtract(a, b) ⊑ a` — a refinement never widens beyond its input.
    #[test]
    fn a_difference_is_below_its_minuend() {
        for a in &universe() {
            for b in &universe() {
                let Some(rest) = subtract(a, b) else {
                    continue;
                };
                assert!(
                    below(&rest, a),
                    "subtract({a}, {b}) = {rest} is not below {a}"
                );
            }
        }
    }

    /// `subtract(a, a)` is bottom: removing a kind from itself leaves nothing.
    #[test]
    fn subtracting_a_kind_from_itself_is_empty() {
        for a in &universe() {
            assert_eq!(subtract(a, a), None, "subtract({a}, {a})");
        }
    }

    /// `subtract(a, b) == a` whenever `a` and `b` are proven disjoint — a fact
    /// about values `a` never held removes nothing from it. This is what keeps
    /// `IF $s != NULL` from touching an `option<string>`.
    #[test]
    fn subtracting_a_disjoint_kind_changes_nothing() {
        for a in &universe() {
            for b in &universe() {
                if !kinds_are_disjoint(a, b) {
                    continue;
                }
                assert_eq!(
                    subtract(a, b),
                    Some(a.clone()),
                    "{b} is disjoint from {a} but subtracting it changed the kind"
                );
            }
        }
    }

    /// `subtract(a, any)` is bottom (`any` covers everything) and
    /// `subtract(any, b)` is `any` (removing a kind from the top has no name,
    /// so the wider answer stands).
    #[test]
    fn subtraction_at_the_top_of_the_lattice() {
        for a in &universe() {
            assert_eq!(subtract(a, &Kind::Any), None, "subtract({a}, any)");
        }
        for b in &universe() {
            if *b == Kind::Any {
                continue;
            }
            assert_eq!(
                subtract(&Kind::Any, b),
                Some(Kind::Any),
                "subtract(any, {b})"
            );
        }
    }

    /// **The safety direction for subtraction.** A difference must admit every
    /// value of `a` that `b` does not. The mechanical check: for every kind `c`
    /// in the universe that is below `a` and disjoint from `b` — i.e. entirely
    /// inside the difference — the reported difference must still admit it, and
    /// must not be bottom.
    #[test]
    fn a_difference_never_drops_a_surviving_kind() {
        let universe = universe();
        for a in &universe {
            for b in &universe {
                let rest = subtract(a, b);
                for c in &universe {
                    if !inhabits(c, a) || !kinds_are_disjoint(c, b) {
                        continue;
                    }
                    let Some(rest) = &rest else {
                        panic!("subtract({a}, {b}) is empty, but {c} inhabits {a} and is disjoint from {b}");
                    };
                    assert!(
                        inhabits(c, rest),
                        "subtract({a}, {b}) = {rest} dropped {c}, which inhabits {a} and is disjoint from {b}"
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // the answers the design turns on
    // -----------------------------------------------------------------------

    /// The §1.3 case: a literal-union field compared to one of its members.
    /// `eq_narrow` gets this wrong today because it compares base kinds for
    /// `==`; as a meet it needs no special case at all.
    #[test]
    fn meeting_a_literal_union_with_one_of_its_members_selects_it() {
        let status = Kind::either(vec![
            string_literal("active"),
            string_literal("inactive"),
            string_literal("banned"),
        ]);
        assert_eq!(
            meet(&status, &string_literal("active")),
            KindMeet::Exact(string_literal("active"))
        );
        // A member the union does not contain is a proven contradiction.
        assert_eq!(meet(&status, &string_literal("bogus")), KindMeet::Empty);
        // And a bare `string` — which is what inference produces for a written
        // literal — narrows the union to nothing but must not widen it either.
        assert_eq!(
            meet(&status, &Kind::String),
            KindMeet::Exact(status.clone())
        );
        // The plain-field case that already works, for the same operation.
        assert_eq!(
            meet(&Kind::String, &string_literal("bob")),
            KindMeet::Exact(string_literal("bob"))
        );
    }

    /// The §1.5 divergence: `!= NONE` and `!= NULL` are different facts, and
    /// one subtraction gets both right because it removes exactly the variant
    /// it was given. `strip_variant` and `narrow_out_none` disagree here today,
    /// and one of them ships a type the database can violate.
    #[test]
    fn the_two_sentinels_subtract_independently() {
        let nullable = Kind::either(vec![Kind::None, Kind::Null, Kind::String]);

        // A NULL survives `!= NONE`.
        assert_eq!(
            subtract(&nullable, &Kind::None),
            Some(Kind::either(vec![Kind::Null, Kind::String]))
        );
        // A NONE survives `!= NULL`.
        assert_eq!(
            subtract(&nullable, &Kind::Null),
            Some(Kind::either(vec![Kind::None, Kind::String]))
        );
        // Only removing both leaves the bare payload.
        assert_eq!(
            subtract(&nullable, &Kind::None).and_then(|rest| subtract(&rest, &Kind::Null)),
            Some(Kind::String)
        );
        // And on a plain option there is no NULL to lose.
        assert_eq!(
            subtract(&option_of(Kind::String), &Kind::None),
            Some(Kind::String)
        );
        assert_eq!(
            subtract(&option_of(Kind::String), &Kind::Null),
            Some(option_of(Kind::String))
        );
    }

    /// Record links narrow by table set in both directions — the discriminant
    /// case (`type::table(p) = 'user'` and its negation), which today is two
    /// separate transforms.
    #[test]
    fn record_table_sets_meet_and_subtract() {
        let both = record(&["user", "folder"]);
        assert_eq!(
            meet(&both, &record(&["user"])),
            KindMeet::Exact(record(&["user"]))
        );
        assert_eq!(
            subtract(&both, &record(&["folder"])),
            Some(record(&["user"]))
        );
        assert_eq!(subtract(&both, &record(&["user", "folder"])), None);
        // A table the link cannot point at is a proven contradiction.
        assert_eq!(
            meet(&record(&["user"]), &record(&["post"])),
            KindMeet::Empty
        );
        // `record<>` is any record: it is the top of the record sub-lattice.
        assert_eq!(meet(&both, &record(&[])), KindMeet::Exact(both.clone()));
        assert_eq!(subtract(&both, &record(&[])), None);
        // Through an `option`, the payload narrows and the sentinel stays.
        assert_eq!(
            meet(&option_of(both.clone()), &record(&["user"])),
            KindMeet::Exact(record(&["user"]))
        );
    }

    /// The over-approximations, pinned as behaviour rather than left to a
    /// comment: each of these is a place the exact answer is not expressible
    /// and the wider one is returned.
    #[test]
    fn the_deliberate_over_approximations() {
        // Removing one value from an infinite kind has no name.
        assert_eq!(
            subtract(&Kind::String, &string_literal("a")),
            Some(Kind::String)
        );
        // The numeric tower is a coercion order, so this is not `float | decimal`.
        assert_eq!(subtract(&Kind::Number, &Kind::Int), Some(Kind::Number));
        // `int` is below both `float` and `decimal` under that same order.
        assert_eq!(meet(&Kind::Int, &Kind::Float), KindMeet::Exact(Kind::Int));
        assert_eq!(
            meet(&Kind::Float, &Kind::Decimal),
            KindMeet::Unrepresentable
        );
        // Two collections with disjoint elements share only the empty one.
        assert_eq!(
            meet(
                &Kind::Array(Box::new(Kind::Int), None),
                &Kind::Array(Box::new(Kind::String), None)
            ),
            KindMeet::Exact(Kind::Array(Box::new(Kind::Any), Some(0)))
        );
        // An array is never a set, whatever they hold.
        assert_eq!(
            meet(
                &Kind::Array(Box::new(Kind::String), None),
                &Kind::Set(Box::new(Kind::String), None)
            ),
            KindMeet::Empty
        );
        // A length is an upper bound, so lengths meet at the smaller one.
        assert_eq!(
            meet(
                &Kind::Array(Box::new(Kind::String), Some(3)),
                &Kind::Array(Box::new(Kind::String), Some(5))
            ),
            KindMeet::Exact(Kind::Array(Box::new(Kind::String), Some(3)))
        );
    }

    /// Closed object literals meet on their shared properties, and a property
    /// one side requires and the other never declares is a proof of
    /// disjointness — the reading `object_is_assignable_to` already takes.
    #[test]
    fn object_literals_meet_structurally() {
        // A required property the other side does not declare: disjoint.
        assert_eq!(
            meet(
                &object(&[("name", Kind::String)]),
                &object(&[("age", Kind::Int)])
            ),
            KindMeet::Empty
        );
        // An OPTIONAL extra property can simply be absent, so the two overlap
        // on the shared property.
        assert_eq!(
            meet(
                &object(&[("name", Kind::String)]),
                &object(&[("name", Kind::String), ("nick", option_of(Kind::String))])
            ),
            KindMeet::Exact(object(&[("name", Kind::String)]))
        );
        // Shared properties meet property-wise — including the literal rule,
        // which is why this is not simply "whichever side came first".
        assert_eq!(
            meet(
                &object(&[("name", Kind::String)]),
                &object(&[("name", string_literal("active"))])
            ),
            KindMeet::Exact(object(&[("name", string_literal("active"))]))
        );
        // Two irreconcilable declarations of one property: disjoint.
        assert_eq!(
            meet(
                &object(&[("name", Kind::String)]),
                &object(&[("name", Kind::Int)])
            ),
            KindMeet::Empty
        );
    }
}
