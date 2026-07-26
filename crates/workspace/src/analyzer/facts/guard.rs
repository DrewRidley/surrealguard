//! `Guard` — the predicate IR, and the one function that builds it.
//!
//! Layer 1 answered *what does this expression denote?* This layer answers the
//! second question: **what does this boolean expression claim about it?**
//!
//! Thirteen recognizers spread across `flow/narrow.rs`, `expression/infer.rs`
//! and `data/select.rs` each answer a slice of it — one for `= NONE` on a
//! param, another for `!= NONE` on a row field, a third for
//! `type::table(x) = 'lit'`, a fourth for `type::is_record`, and each with its
//! own idea of which operand orders, which spellings and which polarities
//! count. They agree by accident; `39_composition.surql` and
//! `36_sentinel_spellings.surql` in the corpus record where they do not.
//!
//! [`guard_of`] is the one function, and [`Atom`] the closed set of claims it
//! can produce. Two properties make the collapse work:
//!
//! * **Negation normal form.** `guard_of(e, false)` is `guard_of(e, true)`
//!   negated *by construction*. De Morgan is applied once, here, rather than
//!   in each consumer, so `!(a = NONE OR b = NONE)` and `a != NONE AND
//!   b != NONE` produce the identical guard — which is what makes the two
//!   halves of a `flip_*` pair impossible to get out of step.
//! * **Polarity folded into the atom.** `IsNone` and `IsNotNone` are separate
//!   atoms rather than one atom plus a sign, so refinement is total and no
//!   consumer can forget to flip.
//!
//! What this file does *not* do is decide anything: an atom is a claim, not a
//! kind. Turning a claim into a refinement is [`super::refine`], and it needs
//! the kinds currently in force, which this file never sees.
//!
//! **Prove or stay silent.** Anything not recognized is [`Guard::Unknown`],
//! which contributes nothing. A guard is allowed to be incomplete; it is never
//! allowed to claim something the expression does not say.

use std::collections::BTreeSet;

use surrealdb_types::{Kind, Table};
use surrealguard_syntax::ast;

use crate::statement_env::StatementEnv;

use super::place::{place_of, Place};
use super::term::{eval, Bindings, ConstValue, Term};

/// An ordering comparison, always written with the place on the left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OrdOp {
    /// `place > x`
    Gt,
    /// `place >= x`
    GtEq,
    /// `place < x`
    Lt,
    /// `place <= x`
    LtEq,
}

impl OrdOp {
    /// The operator with its operands kept but the place moved to the left:
    /// `18 < f` is `f > 18`.
    fn flipped(self) -> OrdOp {
        match self {
            OrdOp::Gt => OrdOp::Lt,
            OrdOp::GtEq => OrdOp::LtEq,
            OrdOp::Lt => OrdOp::Gt,
            OrdOp::LtEq => OrdOp::GtEq,
        }
    }

    /// The operator the *negation* of this comparison asserts. Total over the
    /// four, which is why an ordering guard narrows in both polarities where a
    /// hand-written recognizer only ever handled one.
    fn negated(self) -> OrdOp {
        match self {
            OrdOp::Gt => OrdOp::LtEq,
            OrdOp::GtEq => OrdOp::Lt,
            OrdOp::Lt => OrdOp::GtEq,
            OrdOp::LtEq => OrdOp::Gt,
        }
    }
}

/// Where a membership atom's collection comes from.
///
/// Two shapes, because two are all that can be resolved without inventing:
/// a collection written out in the source, whose element kind is a fact about
/// the text, and a collection named by a place, whose element kind is a
/// question for the environment.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Collection {
    /// An array literal whose elements are constants: the element kind is the
    /// union of their singletons, known without an environment.
    Elements(Kind),
    /// A place holding an `array<E>` / `set<E>`. `E` is resolved against the
    /// kind oracle when the guard is interpreted.
    Of(Place),
}

/// One indivisible claim about one place.
///
/// Closed on purpose. A new spelling of an existing fact costs one arm in
/// [`guard_of`]; a genuinely new fact costs a variant here *and* a refinement
/// in [`super::refine`], which is the point — a claim with no stated
/// refinement cannot be added by accident.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Atom {
    /// `p = NONE`, `p IS NONE`, `type::is_none(p)`.
    IsNone(Place),
    /// `p != NONE`, `p IS NOT NONE`, `!type::is_none(p)`.
    ///
    /// Eliminates the `NONE` and **only** the `NONE`: `NULL = NONE` is FALSE
    /// on the engine, so a NULL survives this guard.
    IsNotNone(Place),
    /// `p = NULL`, `p IS NULL`, `type::is_null(p)`.
    IsNull(Place),
    /// `p != NULL`, `p IS NOT NULL` — a NONE survives.
    IsNotNull(Place),
    /// `p` used as a truthiness guard: `IF p`, `WHERE p`, the right of an
    /// `AND`. Strictly stronger than "not NONE and not NULL", but only the
    /// option markers and a literal `false` are ruled out — see
    /// [`super::refine`].
    Truthy(Place),
    /// `p = <const>`.
    Eq(Place, ConstValue),
    /// `p != <const>`.
    NotEq(Place, ConstValue),
    /// `p <op> <x>`, normalized so the place is on the left. The operand is
    /// **not** carried: the refinement an ordering proves is about the option
    /// markers (`NONE` is the lowest value, so `p > x` rules it out), and it
    /// is the same whatever `x` is — provided `x` is itself not a sentinel,
    /// which [`guard_of`] proves before building the atom.
    Ord(Place, OrdOp),
    /// The place's runtime kind is `k` — `type::is_string(p)` and family.
    HasKind(Place, Kind),
    /// The place's runtime kind is not `k`.
    NotKind(Place, Kind),
    /// The place is a record whose table is in this set: `type::table(p) = 'a'`,
    /// `record::tb(p) = 'a'`, `type::is_record(p, 'a')`.
    InTables(Place, BTreeSet<String>),
    /// The place is a record whose table is *not* in this set.
    NotInTables(Place, BTreeSet<String>),
    /// `p IN <coll>`, `<coll> CONTAINS p`, `p INSIDE <coll>`.
    Member(Place, Collection),
}

impl Atom {
    /// The place this atom claims something about.
    pub(crate) fn place(&self) -> &Place {
        match self {
            Atom::IsNone(place)
            | Atom::IsNotNone(place)
            | Atom::IsNull(place)
            | Atom::IsNotNull(place)
            | Atom::Truthy(place)
            | Atom::Eq(place, _)
            | Atom::NotEq(place, _)
            | Atom::Ord(place, _)
            | Atom::HasKind(place, _)
            | Atom::NotKind(place, _)
            | Atom::InTables(place, _)
            | Atom::NotInTables(place, _)
            | Atom::Member(place, _) => place,
        }
    }
}

/// What a boolean expression asserts.
///
/// Negation-normal: `Not` is pushed into the atoms during construction, so no
/// consumer ever sees one.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Guard {
    /// Every conjunct holds.
    All(Vec<Guard>),
    /// At least one disjunct holds.
    Any(Vec<Guard>),
    /// One claim.
    Atom(Atom),
    /// Provably true. The constant folder's result, lifted into the same IR so
    /// a constant guard is not a special case.
    True,
    /// Provably false.
    False,
    /// Nothing is known. Distinct from [`Guard::True`]/[`Guard::False`]: an
    /// `All` containing `Unknown` still contributes its other conjuncts, and
    /// an `Any` containing one contributes nothing at all.
    Unknown,
}

/// Lower a boolean expression to the guard it asserts.
///
/// `polarity` selects the region: `true` for where the expression is truthy,
/// `false` for where it is falsy. `env`, when present, resolves a `LET`-bound
/// indirect discriminant (`LET $t = type::table($x); IF $t = 'user'`); the row
/// side of a `WHERE` has no environment and passes `None`.
pub(crate) fn guard_of(expr: &ast::Expr, polarity: bool, env: Option<&StatementEnv>) -> Guard {
    // A guard the constant folder settles is `True`/`False` in the same IR,
    // rather than a separate "const path first" branch in every consumer.
    if let Term::Const(ConstValue::Bool(value)) = eval(expr, Bindings::NONE) {
        return if value == polarity {
            Guard::True
        } else {
            Guard::False
        };
    }
    match expr {
        // A parenthesized predicate is the predicate. Grouping parentheses
        // lower to the inner expression today, so this arm is a defence
        // against the lowering changing back — one arm here rather than one
        // bug per recognizer.
        ast::Expr::Subquery(statement) => match &statement.node {
            ast::Statement::Expr(inner) => guard_of(&inner.node, polarity, env),
            _ => Guard::Unknown,
        },
        // De Morgan, applied once.
        ast::Expr::Prefix { op, expr } if matches!(op.node, ast::PrefixOp::Not) => {
            guard_of(&expr.node, !polarity, env)
        }
        ast::Expr::Binary { lhs, op, rhs } => match &op.node {
            ast::BinaryOp::And => compose(polarity, &lhs.node, &rhs.node, env, true),
            ast::BinaryOp::Or => compose(polarity, &lhs.node, &rhs.node, env, false),
            op => binary_guard(&lhs.node, op, &rhs.node, polarity, env),
        },
        ast::Expr::Call(call) => call_guard(call, polarity),
        // A bare place used as a condition is a truthiness test. Its FALSY
        // region proves nothing — `NONE`, `NULL`, `false`, `0`, `''` and `[]`
        // are all falsy, and `Kind` cannot express that set — so the negative
        // polarity is deliberately `Unknown`.
        _ => match (place_of(expr), polarity) {
            (Some(place), true) => Guard::Atom(Atom::Truthy(place)),
            _ => Guard::Unknown,
        },
    }
}

/// `A AND B` under polarity, and its De Morgan dual. `conjunction` is whether
/// the *written* operator is `AND`; the polarity decides which of `All`/`Any`
/// the region is.
fn compose(
    polarity: bool,
    lhs: &ast::Expr,
    rhs: &ast::Expr,
    env: Option<&StatementEnv>,
    conjunction: bool,
) -> Guard {
    let parts = vec![guard_of(lhs, polarity, env), guard_of(rhs, polarity, env)];
    if conjunction == polarity {
        Guard::All(parts)
    } else {
        Guard::Any(parts)
    }
}

/// The claim a single comparison makes, under `polarity`.
fn binary_guard(
    lhs: &ast::Expr,
    op: &ast::BinaryOp,
    rhs: &ast::Expr,
    polarity: bool,
    env: Option<&StatementEnv>,
) -> Guard {
    // `= NONE` / `IS NOT NULL` and the six other sentinel spellings.
    if let Some(equals) = equality_polarity(op) {
        if let Some(atom) = sentinel_atom(lhs, rhs, equals == polarity) {
            return Guard::Atom(atom);
        }
        // `type::table(p) = 'user'` and its two aliases, plus the `LET`-bound
        // indirect form. Tried before the plain literal equality so a
        // discriminant is never mistaken for a value comparison.
        if let Some(atom) = table_atom(lhs, rhs, equals == polarity, env) {
            return Guard::Atom(atom);
        }
        if let Some(atom) = const_eq_atom(lhs, rhs, equals == polarity) {
            return Guard::Atom(atom);
        }
        return Guard::Unknown;
    }
    if let Some(ord) = ordering_op(op) {
        if let Some(atom) = ord_atom(lhs, rhs, ord, polarity) {
            return Guard::Atom(atom);
        }
        return Guard::Unknown;
    }
    match membership(op) {
        // `p IN coll` / `p INSIDE coll`: the member is on the left.
        Some(Membership::In { negated }) if negated != polarity => {
            member_atom(lhs, rhs).map_or(Guard::Unknown, Guard::Atom)
        }
        // `coll CONTAINS p`: the same claim with the operands swapped.
        Some(Membership::Contains { negated }) if negated != polarity => {
            member_atom(rhs, lhs).map_or(Guard::Unknown, Guard::Atom)
        }
        // "not a member" says nothing about the value's kind.
        _ => Guard::Unknown,
    }
}

/// `Some(true)` for the equality family (`=`, `==`, `IS`), `Some(false)` for
/// its negation (`!=`, `IS NOT`).
fn equality_polarity(op: &ast::BinaryOp) -> Option<bool> {
    match op {
        ast::BinaryOp::Eq => Some(true),
        ast::BinaryOp::NotEq => Some(false),
        ast::BinaryOp::Other(text) => match words(text).as_slice() {
            [a] if a.eq_ignore_ascii_case("IS") => Some(true),
            [a, b] if a.eq_ignore_ascii_case("IS") && b.eq_ignore_ascii_case("NOT") => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn ordering_op(op: &ast::BinaryOp) -> Option<OrdOp> {
    Some(match op {
        ast::BinaryOp::Gt => OrdOp::Gt,
        ast::BinaryOp::GtEq => OrdOp::GtEq,
        ast::BinaryOp::Lt => OrdOp::Lt,
        ast::BinaryOp::LtEq => OrdOp::LtEq,
        _ => return None,
    })
}

/// The membership operator a raw operator text spells, if it is one.
enum Membership {
    /// The member is the left operand.
    In { negated: bool },
    /// The member is the right operand.
    Contains { negated: bool },
}

fn membership(op: &ast::BinaryOp) -> Option<Membership> {
    let ast::BinaryOp::Other(text) = op else {
        return None;
    };
    let words = words(text);
    let (negated, name) = match words.as_slice() {
        [name] => (false, *name),
        [not, name] if not.eq_ignore_ascii_case("NOT") => (true, *name),
        _ => return None,
    };
    if name.eq_ignore_ascii_case("IN") || name.eq_ignore_ascii_case("INSIDE") {
        Some(Membership::In { negated })
    } else if name.eq_ignore_ascii_case("CONTAINS") {
        Some(Membership::Contains { negated })
    } else {
        None
    }
}

fn words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

/// `p = NONE` / `p != NULL` and the six other spellings, from either operand
/// order. `holds` is whether the *equality* is what the region asserts.
fn sentinel_atom(lhs: &ast::Expr, rhs: &ast::Expr, holds: bool) -> Option<Atom> {
    let (place, is_none) = match (sentinel(rhs), sentinel(lhs)) {
        (Some(is_none), _) => (place_of(lhs)?, is_none),
        (None, Some(is_none)) => (place_of(rhs)?, is_none),
        (None, None) => return None,
    };
    Some(match (is_none, holds) {
        (true, true) => Atom::IsNone(place),
        (true, false) => Atom::IsNotNone(place),
        (false, true) => Atom::IsNull(place),
        (false, false) => Atom::IsNotNull(place),
    })
}

/// `Some(true)` for `NONE`, `Some(false)` for `NULL`.
fn sentinel(expr: &ast::Expr) -> Option<bool> {
    match eval(expr, Bindings::NONE) {
        Term::Const(ConstValue::None) => Some(true),
        Term::Const(ConstValue::Null) => Some(false),
        _ => None,
    }
}

/// `p = <const>` where the constant is not a sentinel.
fn const_eq_atom(lhs: &ast::Expr, rhs: &ast::Expr, holds: bool) -> Option<Atom> {
    let (place, value) = match (constant(rhs), constant(lhs)) {
        (Some(value), _) => (place_of(lhs)?, value),
        (None, Some(value)) => (place_of(rhs)?, value),
        (None, None) => return None,
    };
    // A constant with no singleton kind (NONE, NULL) is a sentinel and was
    // handled above; one whose kind cannot be written (a datetime, a uuid)
    // refines nothing, so there is no atom to build.
    value.singleton_kind()?;
    Some(if holds {
        Atom::Eq(place, value)
    } else {
        Atom::NotEq(place, value)
    })
}

fn constant(expr: &ast::Expr) -> Option<ConstValue> {
    match eval(expr, Bindings::NONE) {
        Term::Const(value) => Some(value),
        _ => None,
    }
}

/// `type::table(p) = 'user'` — the three spellings of the record-table
/// discriminant, either operand order, plus the `LET`-bound indirect form.
fn table_atom(
    lhs: &ast::Expr,
    rhs: &ast::Expr,
    holds: bool,
    env: Option<&StatementEnv>,
) -> Option<Atom> {
    let of = |disc: &ast::Expr, lit: &ast::Expr| {
        let place = discriminated_place(disc, env)?;
        let table = string_constant(lit)?;
        Some((place, table))
    };
    let (place, table) = of(lhs, rhs).or_else(|| of(rhs, lhs))?;
    let tables = BTreeSet::from([table]);
    Some(if holds {
        Atom::InTables(place, tables)
    } else {
        Atom::NotInTables(place, tables)
    })
}

/// The place a table-discriminant expression projects.
///
/// `type::table(x)`, `record::tb(x)` and `meta::tb(x)` are three names for one
/// function, so all three land here. (`eval` recognizes only the first today,
/// because widening it would move the *old* narrowing path too; when that path
/// goes, this list moves into `Term::Discriminant`'s normalizer where it
/// belongs.)
fn discriminated_place(expr: &ast::Expr, env: Option<&StatementEnv>) -> Option<Place> {
    match expr {
        ast::Expr::Call(call) => {
            let table_projection = matches!(
                call.path.node.as_str(),
                "type::table" | "record::tb" | "meta::tb"
            );
            if !table_projection {
                return None;
            }
            let [arg] = call.args.as_slice() else {
                return None;
            };
            place_of(&arg.node)
        }
        // `LET $t = type::table($x); IF $t = 'user'` — the env records which
        // param a binding discriminates.
        ast::Expr::Param(binding) => {
            let param = env?.table_discriminant(binding)?;
            Some(Place::param(param.to_string()))
        }
        _ => None,
    }
}

fn string_constant(expr: &ast::Expr) -> Option<String> {
    constant(expr)?.as_str().map(str::to_string)
}

/// `p > 18`, normalized so the place is on the left.
///
/// The other operand must be **provably not a sentinel**. That is the whole
/// content of the guard: `NONE` is the lowest value the engine orders and
/// `NULL` the next, so `p > x` rules both out of `p` — but only when `x`
/// cannot itself be one of them (`NULL > NONE` is TRUE).
fn ord_atom(lhs: &ast::Expr, rhs: &ast::Expr, op: OrdOp, polarity: bool) -> Option<Atom> {
    let (place, op) = if let Some(place) = place_of(lhs) {
        (place, op)
    } else if let Some(place) = place_of(rhs) {
        (place, op.flipped())
    } else {
        return None;
    };
    let operand = if place_of(lhs).is_some() { rhs } else { lhs };
    if !is_non_sentinel_operand(operand) {
        return None;
    }
    Some(Atom::Ord(place, if polarity { op } else { op.negated() }))
}

/// Whether an operand is provably neither `NONE` nor `NULL`.
///
/// A written literal is the case that matters, and `eval` cannot answer it
/// alone: its `ConstValue` covers int/float/string/bool and the two sentinels,
/// so a `d'2024-01-01'`, a `2h` or a `u'…'` folds to `Opaque` — indisting-
/// uishable from `$p`, which really might be NONE. Both tests are therefore
/// kept: a foldable constant that is not a sentinel, or a literal token that
/// is not one.
fn is_non_sentinel_operand(expr: &ast::Expr) -> bool {
    match eval(expr, Bindings::NONE) {
        Term::Const(ConstValue::None | ConstValue::Null) => false,
        Term::Const(_) => true,
        _ => matches!(
            expr,
            ast::Expr::Literal(literal)
                if !matches!(literal, ast::Literal::None | ast::Literal::Null)
        ),
    }
}

/// `member IN collection`, with the member already oriented left.
fn member_atom(member: &ast::Expr, collection: &ast::Expr) -> Option<Atom> {
    let place = place_of(member)?;
    Some(Atom::Member(place, collection_of(collection)?))
}

/// The collection a membership guard tests against: an array literal of
/// constants, or a place the oracle can resolve.
fn collection_of(expr: &ast::Expr) -> Option<Collection> {
    // A parenthesized collection is the collection.
    if let ast::Expr::Subquery(statement) = expr {
        if let ast::Statement::Expr(inner) = &statement.node {
            return collection_of(&inner.node);
        }
    }
    if let ast::Expr::Array(elements) = expr {
        let mut kinds = Vec::with_capacity(elements.len());
        for element in elements {
            // One unprovable element and the union is unknown: a member could
            // be that one, so claiming the others' kinds would be a guess.
            kinds.push(constant(&element.node)?.singleton_kind()?);
        }
        // An empty array admits no member at all; there is nothing to prove.
        if kinds.is_empty() {
            return None;
        }
        return Some(Collection::Elements(Kind::either(kinds)));
    }
    place_of(expr).map(Collection::Of)
}

/// The claim a bare predicate call makes: `type::is_string($x)`,
/// `type::is_record($x, 'user')`, `type::is_none($x)`.
///
/// Lowering normalizes `type::is::record` to `type::is_record`, so only the
/// underscore spelling is matched.
fn call_guard(call: &ast::Call, polarity: bool) -> Guard {
    let Some(predicate) = call.path.node.strip_prefix("type::is_") else {
        return Guard::Unknown;
    };
    let Some(arg) = call.args.first() else {
        return Guard::Unknown;
    };
    let Some(place) = place_of(&arg.node) else {
        return Guard::Unknown;
    };
    // `type::is_record($x, 'user')` is the discriminant guard, spelled as a
    // call.
    if predicate == "record" {
        if let Some(table) = call.args.get(1).and_then(|arg| string_constant(&arg.node)) {
            let tables = BTreeSet::from([table]);
            return Guard::Atom(if polarity {
                Atom::InTables(place, tables)
            } else {
                Atom::NotInTables(place, tables)
            });
        }
    }
    // The two sentinel predicates are the sentinel atoms, not kind tests:
    // `type::is_none` is FALSE of a NULL and vice versa, which is exactly the
    // distinction `IsNone`/`IsNull` carry.
    match predicate {
        "none" => {
            return Guard::Atom(if polarity {
                Atom::IsNone(place)
            } else {
                Atom::IsNotNone(place)
            })
        }
        "null" => {
            return Guard::Atom(if polarity {
                Atom::IsNull(place)
            } else {
                Atom::IsNotNull(place)
            })
        }
        _ => {}
    }
    let Some(kind) = predicate_kind(predicate) else {
        return Guard::Unknown;
    };
    Guard::Atom(if polarity {
        Atom::HasKind(place, kind)
    } else {
        Atom::NotKind(place, kind)
    })
}

/// The kind a `type::is_*` predicate proves of its argument.
///
/// Only the predicates whose truth is exactly "inhabits this kind" are listed.
/// `type::is_collection`, `type::is_geometry(x, 'point')` and friends are
/// absent because their answer is a *structural* question rather than a kind,
/// and a wrong entry here would narrow a value the guard does not rule out.
fn predicate_kind(predicate: &str) -> Option<Kind> {
    Some(match predicate {
        "array" => Kind::Array(Box::new(Kind::Any), None),
        "bool" => Kind::Bool,
        "bytes" => Kind::Bytes,
        "datetime" => Kind::Datetime,
        "decimal" => Kind::Decimal,
        "duration" => Kind::Duration,
        "float" => Kind::Float,
        "int" => Kind::Int,
        "number" => Kind::Number,
        "object" => Kind::Object,
        "record" => Kind::Record(Vec::<Table>::new()),
        "set" => Kind::Set(Box::new(Kind::Any), None),
        "string" => Kind::String,
        "uuid" => Kind::Uuid,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::super::place::PlaceRoot;
    use super::*;
    use surrealdb_types::KindLiteral;
    use surrealguard_syntax::lower::lower_first_expr;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    /// The guard of the condition in `IF <source> { RETURN 1; }`.
    fn guard(source: &str, polarity: bool) -> Guard {
        let query = format!("IF {source} {{ RETURN 1; }};");
        let parsed = parse_source(SourceId::new("guard:test"), query.as_str()).expect("parses");
        let statements = surrealguard_syntax::lower::lower_statements(&parsed);
        let ast::Statement::IfElse(stmt) = &statements.first().expect("one statement").node else {
            panic!("expected an IF");
        };
        guard_of(&stmt.branches[0].condition.node, polarity, None)
    }

    fn place(name: &str) -> Place {
        Place::param(name)
    }

    #[test]
    fn a_sentinel_guard_is_one_atom_per_polarity() {
        assert_eq!(
            guard("$x = NONE", true),
            Guard::Atom(Atom::IsNone(place("x")))
        );
        assert_eq!(
            guard("$x = NONE", false),
            Guard::Atom(Atom::IsNotNone(place("x")))
        );
        assert_eq!(
            guard("$x != NULL", true),
            Guard::Atom(Atom::IsNotNull(place("x")))
        );
    }

    #[test]
    fn every_spelling_of_one_sentinel_fact_is_one_atom() {
        // The equivalence class the corpus records, asserted directly.
        for source in [
            "$x != NONE",
            "$x IS NOT NONE",
            "NONE != $x",
            "!($x = NONE)",
            "!!($x != NONE)",
            "($x != NONE)",
            "(($x) != NONE)",
        ] {
            assert_eq!(
                guard(source, true),
                Guard::Atom(Atom::IsNotNone(place("x"))),
                "`{source}` must assert the same fact"
            );
        }
        // …and `type::is_none` is the same fact under the other polarity.
        assert_eq!(
            guard("type::is_none($x)", false),
            Guard::Atom(Atom::IsNotNone(place("x")))
        );
    }

    #[test]
    fn de_morgan_holds_by_construction() {
        // `!(a = NONE OR b = NONE)` IS `a != NONE AND b != NONE`.
        assert_eq!(
            guard("!($a = NONE OR $b = NONE)", true),
            guard("$a != NONE AND $b != NONE", true)
        );
        // …and the mirror, read out of the ELSE.
        assert_eq!(
            guard("!($a != NONE AND $b != NONE)", true),
            guard("$a = NONE OR $b = NONE", true)
        );
        // The negative region of an `AND` is the `Any` of the negations.
        assert_eq!(
            guard("$a != NONE AND $b != NONE", false),
            Guard::Any(vec![
                Guard::Atom(Atom::IsNone(place("a"))),
                Guard::Atom(Atom::IsNone(place("b"))),
            ])
        );
    }

    #[test]
    fn a_constant_condition_is_a_constant_guard() {
        assert_eq!(guard("1 = 1", true), Guard::True);
        assert_eq!(guard("1 = 1", false), Guard::False);
        assert_eq!(guard("(1 = 1)", true), Guard::True);
        assert_eq!(guard("1 + 0 = 1", true), Guard::True);
    }

    #[test]
    fn the_three_table_discriminant_spellings_are_one_atom() {
        let expected = Guard::Atom(Atom::InTables(
            place("x"),
            BTreeSet::from(["user".to_string()]),
        ));
        for source in [
            "type::table($x) = 'user'",
            "record::tb($x) = 'user'",
            "meta::tb($x) = 'user'",
            "'user' = type::table($x)",
            "type::table(($x)) = 'user'",
            "type::is_record($x, 'user')",
        ] {
            assert_eq!(guard(source, true), expected, "`{source}`");
        }
        assert_eq!(
            guard("type::table($x) != 'user'", true),
            guard("type::table($x) = 'user'", false)
        );
    }

    #[test]
    fn the_three_membership_spellings_are_one_atom() {
        let expected = Guard::Atom(Atom::Member(place("x"), Collection::Of(place("pool"))));
        for source in [
            "$x IN $pool",
            "$x INSIDE $pool",
            "$pool CONTAINS $x",
            "$x IN ($pool)",
        ] {
            assert_eq!(guard(source, true), expected, "`{source}`");
        }
        // `NOT IN` is the negation: it proves the membership in the FALSY
        // region and nothing in the truthy one.
        assert_eq!(guard("$x NOT IN $pool", false), expected);
        assert_eq!(guard("$x NOT IN $pool", true), Guard::Unknown);
        // …and a membership that fails proves nothing either way.
        assert_eq!(guard("$x IN $pool", false), Guard::Unknown);
    }

    #[test]
    fn an_array_literal_collection_carries_its_element_union() {
        assert_eq!(
            guard("$x IN ['a', 'b']", true),
            Guard::Atom(Atom::Member(
                place("x"),
                Collection::Elements(Kind::either(vec![
                    Kind::Literal(KindLiteral::String("a".into())),
                    Kind::Literal(KindLiteral::String("b".into())),
                ]))
            ))
        );
        // One unprovable element and the union is a guess, so there is none.
        assert_eq!(guard("$x IN ['a', $y]", true), Guard::Unknown);
    }

    #[test]
    fn an_ordering_guard_needs_a_provably_non_sentinel_operand() {
        assert_eq!(
            guard("$x > 18", true),
            Guard::Atom(Atom::Ord(place("x"), OrdOp::Gt))
        );
        // Operand order is normalized: `18 < $x` is `$x > 18`.
        assert_eq!(guard("18 < $x", true), guard("$x > 18", true));
        // The negation is the complementary operator, not "unknown".
        assert_eq!(
            guard("$x > 18", false),
            Guard::Atom(Atom::Ord(place("x"), OrdOp::LtEq))
        );
        // A literal `eval` cannot fold is still provably not a sentinel.
        assert_eq!(
            guard("$x > d'2024-01-01T00:00:00Z'", true),
            Guard::Atom(Atom::Ord(place("x"), OrdOp::Gt))
        );
        // A param might BE a sentinel — `NULL > NONE` is TRUE — so it proves
        // nothing.
        assert_eq!(guard("$x > $y", true), Guard::Unknown);
    }

    #[test]
    fn a_bare_place_is_a_truthiness_guard_in_one_direction_only() {
        assert_eq!(guard("$x", true), Guard::Atom(Atom::Truthy(place("x"))));
        // A falsy value may be NONE, NULL, `false`, `0`, `''` or `[]`; `Kind`
        // cannot say that, so the negative region proves nothing.
        assert_eq!(guard("$x", false), Guard::Unknown);
        assert_eq!(guard("!$x", false), Guard::Atom(Atom::Truthy(place("x"))));
    }

    #[test]
    fn an_unrecognized_condition_is_unknown_not_a_guess() {
        for source in ["fn::check($x)", "$x.items[WHERE ok] != NONE", "$x + 1 > $y"] {
            assert_eq!(guard(source, true), Guard::Unknown, "`{source}`");
        }
    }

    #[test]
    fn a_kind_predicate_is_a_kind_atom() {
        assert_eq!(
            guard("type::is_string($x)", true),
            Guard::Atom(Atom::HasKind(place("x"), Kind::String))
        );
        assert_eq!(
            guard("type::is_string($x)", false),
            Guard::Atom(Atom::NotKind(place("x"), Kind::String))
        );
        // An unmodeled predicate proves nothing rather than something wrong.
        assert_eq!(guard("type::is_collection($x)", true), Guard::Unknown);
    }

    #[test]
    fn a_literal_equality_is_an_atom_on_either_side() {
        assert_eq!(
            guard("$x = 'draft'", true),
            Guard::Atom(Atom::Eq(place("x"), ConstValue::Str("draft".into())))
        );
        assert_eq!(guard("'draft' = $x", true), guard("$x = 'draft'", true));
        assert_eq!(
            guard("$x = 'draft'", false),
            Guard::Atom(Atom::NotEq(place("x"), ConstValue::Str("draft".into())))
        );
    }

    #[test]
    fn a_row_field_is_a_place_like_any_other() {
        assert_eq!(
            guard_of(
                &where_cond("SELECT * FROM t WHERE email != NONE;"),
                true,
                None
            ),
            Guard::Atom(Atom::IsNotNone(Place {
                root: PlaceRoot::RowField,
                path: vec![super::super::place::Step::Field("email".into())],
            }))
        );
    }

    fn where_cond(query: &str) -> ast::Expr {
        let parsed = parse_source(SourceId::new("guard:test"), query).expect("parses");
        match surrealguard_syntax::lower::lower_first_statement(&parsed, "SelectStatement")
            .expect("select statement exists")
            .node
        {
            ast::Statement::Select(stmt) => stmt.where_clause.expect("has a WHERE").node,
            other => panic!("expected select, got {other:?}"),
        }
    }

    #[test]
    fn lowering_keeps_the_operator_text_this_module_reads() {
        // The membership and `IS` spellings arrive as raw operator text, so a
        // lowering change to that text would silently disable half this file.
        for (source, expected) in [
            ("$x IN $y", "IN"),
            ("$x NOT IN $y", "NOT IN"),
            ("$x INSIDE $y", "INSIDE"),
            ("$x CONTAINS $y", "CONTAINS"),
            ("$x IS NOT NONE", "IS NOT"),
        ] {
            let query = format!("RETURN {source};");
            let parsed = parse_source(SourceId::new("guard:test"), query.as_str()).expect("parses");
            let expr = lower_first_expr(&parsed, "BinaryExpression")
                .expect("a binary expression")
                .node;
            let ast::Expr::Binary { op, .. } = expr else {
                panic!("expected a binary expression for `{source}`");
            };
            assert_eq!(
                op.node,
                ast::BinaryOp::Other(expected.to_string()),
                "`{source}`"
            );
        }
    }
}
