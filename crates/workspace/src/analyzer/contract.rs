//! One contract, checked at every position.
//!
//! A position that requires a kind — a field write, a function argument, a
//! `LIMIT`, an `IF` condition — is one assertion: *the value that lands here
//! inhabits `T`*. The analyzer used to spell that assertion once per position,
//! and the spellings disagreed on five independent axes: whether a written
//! constant is compared as the literal it *is* or as the kind inference
//! widened it to, whether a union distributes, where the `any` short-circuit
//! sits, what an unbound `$param` does, and whether the position is checked at
//! all. `DEFINE FIELD e ON t TYPE 'red' | 'blue' VALUE 'green'` was silent for
//! exactly that reason while `CREATE t SET e = 'green'` reported — one rule,
//! two implementations, one of them wrong.
//!
//! This module is the rule. [`Position`] enumerates every place in SurrealQL
//! that demands a kind, [`Contract`] pairs the demand with the finding it
//! raises, and [`decide`] is the only decision.
//!
//! ## Prove or stay silent
//!
//! [`Verdict`] is three-valued because the policy is. An error-severity
//! finding aborts code generation for a whole workspace, so a check that fires
//! on a value it cannot see is worse than a check that misses one it can:
//!
//! * [`Verdict::Violated`] — *proven* not to inhabit the contract. Emit.
//! * [`Verdict::Satisfied`] — proven to inhabit it. Silent.
//! * [`Verdict::Unknown`] — neither. Silent, deliberately: an `any`, an
//!   unbound `$param`, a call whose result the analyzer does not model.
//!
//! That is why a bare `string` written into a `'red' | 'blue'` field is
//! *accepted*: inference widens a written `'red'` to `string`, so the widened
//! kind carries no evidence about which string it holds. The evidence lives in
//! the *value*, and [`checked_kind`] is what recovers it.

use surrealdb_types::Kind;

use crate::expression::ExpressionFact;
use crate::kinds::kind_is_assignable_to;
use crate::lattice::kinds_are_disjoint;

use super::facts::{ConstValue, Term};

/// Every position in SurrealQL that requires a kind.
///
/// Exhaustive on purpose, and in production code rather than in a test: the
/// contract-position harness iterates [`Position::ALL`], so a new position that
/// forgets its contract is a test failure rather than a paragraph in a plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Position {
    /// `CREATE`/`UPDATE`/`UPSERT`/`RELATE` … `SET f = v`.
    MutationSet,
    /// `CONTENT { f: v }` (and `REPLACE`).
    MutationContent,
    /// `MERGE { f: v }`.
    MutationMerge,
    /// `INSERT INTO t (f) VALUES (v)`.
    InsertValues,
    /// `DEFINE FIELD … VALUE v`.
    FieldValue,
    /// `DEFINE FIELD … DEFAULT v`.
    FieldDefault,
    /// `DEFINE FIELD … COMPUTED v`.
    FieldComputed,
    /// `DEFINE FIELD … ASSERT p`.
    FieldAssert,
    /// Argument *n* of a user-defined `fn::` call.
    FunctionArg,
    /// A `DEFINE FUNCTION … -> T` body.
    FunctionReturn,
    /// `DEFINE PARAM $x VALUE v`.
    ParamDefault,
    /// `LIMIT n`.
    Limit,
    /// `START n`.
    Start,
    /// `TIMEOUT d`.
    Timeout,
    /// `SPLIT f`.
    Split,
    /// `FETCH f`.
    ///
    /// Not a [`Contract`] row, and not because nobody got to it: its rule is
    /// "does this kind hold a record *anywhere* inside it", which recurses
    /// through element kinds (`array<array<record<t>>>` fetches). That is a
    /// structural search, not a kind, so there is no `expects` to write.
    Fetch,
    /// The collection of a `FOR $x IN e`.
    ForIterable,
    /// The operand of a `<type>` cast.
    ///
    /// Not a [`Contract`] row either. A cast does not ask whether the value
    /// inhabits the target — that is the whole point of writing one — it asks
    /// whether the *conversion* can succeed, which is a relation between two
    /// kinds with its own per-target table (`string` converts to `datetime`,
    /// `datetime` does not convert to `int`).
    Cast,
    /// `SELECT … WHERE p`.
    WhereSelect,
    /// `UPDATE`/`UPSERT`/`DELETE … WHERE p`.
    WhereMutation,
    /// `IF p`.
    IfCond,
    /// `DEFINE EVENT … WHEN p`.
    EventWhen,
    /// `PERMISSIONS FOR … WHERE p`.
    PermissionPredicate,
}

impl Position {
    /// Every position, in declaration order. The harness quantifies over this.
    pub const ALL: &'static [Position] = &[
        Position::MutationSet,
        Position::MutationContent,
        Position::MutationMerge,
        Position::InsertValues,
        Position::FieldValue,
        Position::FieldDefault,
        Position::FieldComputed,
        Position::FieldAssert,
        Position::FunctionArg,
        Position::FunctionReturn,
        Position::ParamDefault,
        Position::Limit,
        Position::Start,
        Position::Timeout,
        Position::Split,
        Position::Fetch,
        Position::ForIterable,
        Position::Cast,
        Position::WhereSelect,
        Position::WhereMutation,
        Position::IfCond,
        Position::EventWhen,
        Position::PermissionPredicate,
    ];

    /// The finding this position raises when its contract is violated.
    ///
    /// The mapping lives here rather than at each call site because it is
    /// one-to-one and the harness needs it too: a position and the code it
    /// raises are the same fact, and spelling it twice is how a check ends up
    /// tested against a code it no longer emits.
    pub fn code(self) -> u16 {
        match self {
            Position::MutationSet
            | Position::MutationContent
            | Position::MutationMerge
            | Position::InsertValues
            | Position::FieldValue
            | Position::FieldDefault
            | Position::FieldComputed
            | Position::ParamDefault => 2001,
            Position::FieldAssert
            | Position::WhereSelect
            | Position::WhereMutation
            | Position::IfCond
            | Position::EventWhen
            | Position::PermissionPredicate => 2005,
            Position::FunctionArg => 5002,
            Position::FunctionReturn => 2012,
            Position::Limit | Position::Start => 2018,
            Position::Timeout => 2019,
            Position::Split => 1024,
            Position::Fetch => 1023,
            Position::ForIterable => 2022,
            Position::Cast => 2008,
        }
    }

    /// The position's name, for a harness failure message.
    pub fn name(self) -> &'static str {
        match self {
            Position::MutationSet => "MutationSet",
            Position::MutationContent => "MutationContent",
            Position::MutationMerge => "MutationMerge",
            Position::InsertValues => "InsertValues",
            Position::FieldValue => "FieldValue",
            Position::FieldDefault => "FieldDefault",
            Position::FieldComputed => "FieldComputed",
            Position::FieldAssert => "FieldAssert",
            Position::FunctionArg => "FunctionArg",
            Position::FunctionReturn => "FunctionReturn",
            Position::ParamDefault => "ParamDefault",
            Position::Limit => "Limit",
            Position::Start => "Start",
            Position::Timeout => "Timeout",
            Position::Split => "Split",
            Position::Fetch => "Fetch",
            Position::ForIterable => "ForIterable",
            Position::Cast => "Cast",
            Position::WhereSelect => "WhereSelect",
            Position::WhereMutation => "WhereMutation",
            Position::IfCond => "IfCond",
            Position::EventWhen => "EventWhen",
            Position::PermissionPredicate => "PermissionPredicate",
        }
    }
}

/// The decision [`check`] returns. Three-valued because the policy is — see
/// the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// The value provably inhabits the contract.
    Satisfied,
    /// The value is *proven* not to inhabit it. The only verdict that emits.
    Violated,
    /// Not provable either way. Silent.
    Unknown,
}

impl Verdict {
    /// Whether this verdict emits the contract's finding.
    pub(crate) fn is_violation(self) -> bool {
        matches!(self, Verdict::Violated)
    }
}

/// How a position folds the case that is neither proof: the value overlaps
/// what the position admits without being wholly inside it.
///
/// Both folds already existed in the analyzer, unnamed and one per site, and
/// the reason there are two is not sloppiness — the positions genuinely differ:
///
/// * A field write, a declared return and a function argument demand
///   *inhabitation*. `SET n = <a number>` into a `TYPE int` field is reported
///   today and should be: the write is only correct if every value the
///   expression can produce fits.
/// * A condition and a clause demand *possibility*. SurrealQL reads a
///   `bool | string` as a condition happily, and a `LIMIT` accepts the
///   `number` that `math::floor` returns. Reporting those would be a false
///   positive on idiomatic input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Strictness {
    /// A partial overlap is a violation: the value must inhabit `expects`.
    Inhabits,
    /// Only a proven-disjoint kind is a violation: the value must merely be
    /// able to be one `expects` admits.
    Possible,
}

/// A position that demands a kind: what it admits, and what it raises when a
/// value is proven not to inhabit it.
///
/// Constructed per call site because `expects` comes from the schema (a
/// field's declared type, a function's declared return), but the *decision* is
/// shared. Sites keep their own message: a contract knows what was violated,
/// not how to say it.
#[derive(Clone, Debug)]
pub(crate) struct Contract {
    /// Which position this is. It supplies [`Contract::code`] and names the
    /// row for a reader; [`decide`] does not branch on it, which is the point.
    pub position: Position,
    /// The kind the position admits.
    pub expects: Kind,
    /// How a partial overlap is folded.
    pub strictness: Strictness,
}

impl Contract {
    /// A contract for `position` admitting `expects`. The value must
    /// *inhabit* `expects`; the code comes from the position.
    pub(crate) fn new(position: Position, expects: Kind) -> Self {
        Self {
            position,
            expects,
            strictness: Strictness::Inhabits,
        }
    }

    /// A contract that reports only what it can prove disjoint — see
    /// [`Strictness::Possible`].
    pub(crate) fn possible(position: Position, expects: Kind) -> Self {
        Self {
            strictness: Strictness::Possible,
            ..Self::new(position, expects)
        }
    }

    /// The contract every condition position shares (2005).
    ///
    /// `bool | none | null` rather than `bool`, because SurrealQL reads the two
    /// sentinels as falsy — a `WHERE $maybe` over an `option<bool>` is
    /// idiomatic, not a mistake. And `Possible`, because a `bool | string`
    /// really can be a bool at runtime; only a kind that can *never* be read as
    /// a condition is reported.
    pub(crate) fn condition(position: Position) -> Self {
        Self::possible(
            position,
            Kind::Either(vec![Kind::Bool, Kind::None, Kind::Null]),
        )
    }

    /// The finding this contract raises, from its position.
    pub(crate) fn code(&self) -> u16 {
        self.position.code()
    }

    /// The verdict for a value already reduced to a kind — the Group C
    /// positions, which have no value to recover (a `SPLIT` names a field, not
    /// a constant) and the whole-body comparisons.
    pub(crate) fn decide(&self, actual: &Kind) -> Verdict {
        decide(actual, &self.expects, self.strictness)
    }

    /// The kind a violation was decided at, or `None` when the contract holds
    /// or nothing could be proven either way.
    ///
    /// This is the shape every position actually wants: the check and the kind
    /// its message has to name are one question, and returning them separately
    /// is how a site ends up rendering something other than what it compared.
    pub(crate) fn violation(&self, term: &Term, fact: &ExpressionFact) -> Option<Kind> {
        let actual = checked_kind(term, fact)?;
        self.decide(&actual).is_violation().then_some(actual)
    }
}

/// Whether a value of kind `actual` inhabits `expects`.
///
/// `Violated` is a proof, and it has two independent sources, because neither
/// subsumes the other under the order this analyzer uses:
///
/// * [`kinds_are_disjoint`] — a [`crate::lattice::meet`] of `⊥`: no value is at
///   once a `string` and an `int`, and no value is at once `'green'` and one of
///   `'red' | 'blue'`. This is the half that closes the literal hole.
/// * `!`[`kind_is_assignable_to`] — the subtype test. It is *stricter* than
///   disjointness on the numeric tower, on purpose and with tests behind it:
///   `meet(float, int)` is `int` rather than `⊥`, because the lattice's order
///   reads `int` into `float` as a coercion, so a `TYPE int VALUE 1.5` is not
///   provably disjoint under the meet alone. It is still wrong, and reporting
///   it is shipped behaviour.
///
/// The third case — a real overlap that is not containment — is neither proof,
/// and [`Strictness`] is which way the position folds it.
///
/// `Any` on the value side proves nothing in either direction and is the single
/// most common source of a false positive, so it short-circuits to `Unknown`
/// here rather than being spelled `kind != Kind::Any` at each of twenty sites.
pub(crate) fn decide(actual: &Kind, expects: &Kind, strictness: Strictness) -> Verdict {
    if matches!(actual, Kind::Any) {
        return Verdict::Unknown;
    }
    if kinds_are_disjoint(actual, expects) {
        return Verdict::Violated;
    }
    if kind_is_assignable_to(actual, expects) {
        return Verdict::Satisfied;
    }
    match strictness {
        Strictness::Inhabits => Verdict::Violated,
        Strictness::Possible => Verdict::Unknown,
    }
}

/// The kind an expression must be *checked* at: the value it provably is, when
/// something proved one, and the kind inference gave it otherwise.
///
/// Inference widens a written `'green'` to `string`, which is right for
/// building a response type and wrong for checking a contract — a `string`
/// can never be shown to fall outside `'red' | 'blue'`, so the wrong value
/// slips through. Recovering the exact literal is what makes the check
/// *provable* rather than merely present.
///
/// Two sources of a proven value, and neither subsumes the other:
///
/// * [`Term::Const`] — the folder. It sees through parentheses and folds
///   arithmetic, comparisons and membership (`1 + 0`, `'a' IN ['a']`), none of
///   which reaches [`ExpressionFact::value`].
/// * [`ExpressionFact::value`] — inference's own constant tracking. It traces
///   `LET` indirection (`LET $s = 'green'; CREATE t SET e = $s`) and carries
///   durations and decimals, none of which the folder models (it declines to
///   resolve a `LET` on purpose: its other callers grey code on the result).
///
/// So the folder is asked first and inference's value is the fallback. Neither
/// widens anything: when nothing proved a value, the inferred kind is returned
/// unchanged and the position stays as precise as it was.
pub(crate) fn checked_kind(term: &Term, fact: &ExpressionFact) -> Option<Kind> {
    if let Some(folded) = term_kind(term) {
        return Some(folded);
    }
    if let Some(exact) = fact
        .value
        .as_ref()
        .and_then(crate::kinds::scalar_value_literal_kind)
    {
        return Some(exact);
    }
    fact.kind.clone()
}

/// The singleton kind the folder proved, when it proved one. The half of
/// [`checked_kind`] available to a position that holds the expression but not
/// its fact.
pub(crate) fn term_kind(term: &Term) -> Option<Kind> {
    match term {
        Term::Const(value) => Some(const_kind(value)),
        _ => None,
    }
}

/// The singleton kind a folded constant inhabits. The sentinels are their own
/// kinds rather than literals of one.
fn const_kind(value: &ConstValue) -> Kind {
    match value {
        ConstValue::None => Kind::None,
        ConstValue::Null => Kind::Null,
        // Every remaining variant has a literal kind.
        other => other.singleton_kind().unwrap_or(Kind::Any),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealdb_types::KindLiteral;

    fn literal(text: &str) -> Kind {
        Kind::Literal(KindLiteral::String(text.to_string()))
    }

    fn union(variants: Vec<Kind>) -> Kind {
        Kind::Either(variants)
    }

    #[test]
    fn a_known_value_outside_a_literal_union_is_proven_wrong() {
        let expects = union(vec![literal("red"), literal("blue")]);
        assert_eq!(
            decide(&literal("green"), &expects, Strictness::Inhabits),
            Verdict::Violated
        );
        assert_eq!(
            decide(&literal("red"), &expects, Strictness::Inhabits),
            Verdict::Satisfied
        );
    }

    #[test]
    fn a_widened_string_against_the_same_union_stays_silent() {
        // The half that keeps this from being a false-positive wave: a value
        // whose *kind* is `string` carries no evidence about which string, so
        // it is not provably outside the union.
        let expects = union(vec![literal("red"), literal("blue")]);
        assert_eq!(
            decide(&Kind::String, &expects, Strictness::Inhabits),
            Verdict::Satisfied
        );
    }

    #[test]
    fn any_proves_nothing_on_the_value_side() {
        assert_eq!(
            decide(&Kind::Any, &Kind::Int, Strictness::Inhabits),
            Verdict::Unknown
        );
        // …but an `any` *expectation* accepts anything.
        assert_eq!(
            decide(&Kind::Int, &Kind::Any, Strictness::Inhabits),
            Verdict::Satisfied
        );
    }

    #[test]
    fn the_numeric_tower_stays_as_strict_as_it_shipped() {
        // `meet(float, int)` is `int`, not `⊥` — the lattice reads `int` into
        // `float` as a coercion — so disjointness alone would let a
        // `TYPE int VALUE 1.5` through. The subtype test is what catches it.
        assert_eq!(
            decide(&Kind::Float, &Kind::Int, Strictness::Inhabits),
            Verdict::Violated
        );
        assert_eq!(
            decide(&Kind::Int, &Kind::Float, Strictness::Inhabits),
            Verdict::Satisfied
        );
    }

    #[test]
    fn an_optional_target_accepts_the_sentinel_and_a_bare_one_does_not() {
        let optional = union(vec![Kind::None, Kind::Int]);
        assert_eq!(
            decide(&Kind::None, &optional, Strictness::Inhabits),
            Verdict::Satisfied
        );
        assert_eq!(
            decide(&Kind::None, &Kind::Int, Strictness::Inhabits),
            Verdict::Violated
        );
    }
}
