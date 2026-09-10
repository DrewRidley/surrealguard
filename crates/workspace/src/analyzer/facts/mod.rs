//! The expression-fact layer: what an expression *denotes*, computed once.
//!
//! Every narrowing recognizer, every constant folder and every "is this a
//! `$param.field` path?" test in this analyzer conflates three separable
//! questions:
//!
//! 1. **Denotation** — which location or value does this expression name?
//!    `(x)`, `x` and `$x.f` after a `LET` alias all name the same thing;
//!    `type::table(f)` names a projection of one.
//! 2. **Assertion** — what does this boolean expression claim about it?
//! 3. **Refinement** — what does that claim do to a `Kind`?
//!
//! This module owns (1), and it is the only part of the analyzer that should
//! pattern-match `ast::Expr` to answer it. (3) is [`crate::lattice`]. (2) is
//! the guard IR, which lands on top of these two.
//!
//! Nothing here decides anything: `place_of` and `eval` are normalizations, so
//! a consumer that reads them keeps its own policy about *which* places and
//! values it is willing to act on.

pub(crate) mod assert;
pub(crate) mod guard;
pub(crate) mod place;
pub(crate) mod refine;
pub(crate) mod term;

pub(crate) use assert::constant_violates_assert;
pub(crate) use guard::{guard_of, Guard};
pub(crate) use place::{place_of, Place, PlaceRoot};
pub(crate) use refine::{refined_under, Facts, KindOracle, Refinement, RootedKind, Verdict};
pub(crate) use term::{eval, Bindings, ConstValue, DiscriminantKind, Term};
