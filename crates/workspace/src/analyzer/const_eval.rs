//! Branch reachability: which arms of an `IF`/`ELSE` chain can run.
//!
//! Nothing here recognizes an expression. Deciding what a condition proves is
//! [`crate::analyzer::facts`]'s job — one guard IR, one interpreter — and what
//! stays in this module is the *policy*: how a decided guard turns into a
//! reachability verdict, which is a statement about control flow rather than
//! about expressions.
//!
//! The layer's contract governs everything below: it decides only what it can
//! prove and answers `Verdict::Unknown` the instant an operand is
//! runtime-dependent, so greying a branch never rests on a guess.

use surrealguard_syntax::ast;

use crate::analyzer::facts::{KindOracle, Place, Verdict};

/// An oracle that knows nothing, so no claim about a place can be decided
/// through it. What makes [`branch_reachability`] constant-only a property of
/// the environment it is given rather than of which shapes it happens to
/// recognize.
struct Nothing;

impl KindOracle for Nothing {
    fn kind_of(&self, _place: &Place) -> Option<surrealdb_types::Kind> {
        None
    }
}

/// Whether one `IF`/`ELSE IF` arm can be reached at runtime, and — when it
/// cannot — *why*, so callers can word the greying finding precisely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchReach {
    /// The guard is unknown, or is the first provably-true guard: this branch
    /// can run.
    Reachable,
    /// The guard provably folds to `false`: this branch is never taken.
    DeadFalse,
    /// An earlier guard provably folds to `true`: control never reaches here.
    DeadAfterTrue,
}

impl BranchReach {
    /// Whether control can reach this branch's body.
    pub fn is_reachable(self) -> bool {
        matches!(self, BranchReach::Reachable)
    }
}

/// The reachability of every arm of an `IF`/`ELSE` chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reachability {
    /// One entry per `IF`/`ELSE IF` branch, in source order.
    pub branches: Vec<BranchReach>,
    /// Whether the `ELSE` (or, absent one, the implicit `NONE` fall-through) is
    /// unreachable — true exactly when some earlier branch is provably taken.
    pub else_dead: bool,
}

/// Walks the branches of an `IF`/`ELSE` in order, folding each guard:
///
/// * a guard that provably folds to `false` → the branch is [`DeadFalse`];
/// * the first guard that provably folds to `true` → that branch is
///   [`Reachable`], and every branch after it plus the `ELSE` are dead
///   ([`DeadAfterTrue`] / `else_dead`);
/// * an unfoldable (unknown) guard → the branch stays [`Reachable`] and does
///   *not* make later arms dead.
///
/// Decides only constants: read through an oracle that knows nothing, a guard
/// mentioning any param, field, function, or idiom is unknown, so its branch is
/// kept.
///
/// [`DeadFalse`]: BranchReach::DeadFalse
/// [`DeadAfterTrue`]: BranchReach::DeadAfterTrue
/// [`Reachable`]: BranchReach::Reachable
pub fn branch_reachability(stmt: &ast::IfElseStmt) -> Reachability {
    let mut branches = Vec::with_capacity(stmt.branches.len());
    // Set once an earlier branch is proven always-taken.
    let mut taken = false;
    for branch in &stmt.branches {
        if taken {
            branches.push(BranchReach::DeadAfterTrue);
            continue;
        }
        // The guard IR, read with an oracle that knows nothing. A constant
        // guard lowered to `Guard::True`/`Guard::False`, so the fold is not a
        // special case here either; and because the oracle answers nothing,
        // every non-constant claim is `Unknown` — which is what keeps this
        // function const-only, by construction rather than by omission.
        match crate::analyzer::facts::guard_of(&branch.condition.node, true, None).verdict(&Nothing)
        {
            Verdict::AlwaysFalse => branches.push(BranchReach::DeadFalse),
            Verdict::AlwaysTrue => {
                branches.push(BranchReach::Reachable);
                taken = true;
            }
            Verdict::Unknown => branches.push(BranchReach::Reachable),
        }
    }
    Reachability {
        branches,
        else_dead: taken,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_syntax::ast;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    /// Builds the first `IfElseStatement` in `source`.
    fn reachability_of(source: &str) -> Reachability {
        let parsed = parse_source(SourceId::new("const:test"), source).expect("query parses");
        let ast::Statement::IfElse(stmt) =
            surrealguard_syntax::lower::lower_first_statement(&parsed, "IfElseStatement")
                .expect("no IfElseStatement node")
                .node
        else {
            panic!("expected if statement");
        };
        branch_reachability(&stmt)
    }

    #[test]
    fn const_true_guard_kills_the_else() {
        let reach = reachability_of("IF 1 == 1 { RETURN 1 } ELSE { RETURN 2 };");
        assert_eq!(reach.branches, vec![BranchReach::Reachable]);
        assert!(reach.else_dead);
    }

    #[test]
    fn const_false_guard_is_dead_but_else_lives() {
        let reach = reachability_of("IF 2 > 3 { RETURN 1 } ELSE { RETURN 2 };");
        assert_eq!(reach.branches, vec![BranchReach::DeadFalse]);
        assert!(!reach.else_dead);
    }

    #[test]
    fn branch_after_a_const_true_is_dead() {
        let reach =
            reachability_of("IF false { RETURN 0 } ELSE IF true { RETURN 1 } ELSE { RETURN 2 };");
        assert_eq!(
            reach.branches,
            vec![BranchReach::DeadFalse, BranchReach::Reachable]
        );
        assert!(reach.else_dead);
    }

    #[test]
    fn unknown_guard_keeps_everything_reachable() {
        let reach = reachability_of("IF $x { RETURN 1 } ELSE { RETURN 2 };");
        assert_eq!(reach.branches, vec![BranchReach::Reachable]);
        assert!(!reach.else_dead);
    }

    #[test]
    fn an_unknown_guard_does_not_kill_a_later_const_true() {
        // The unknown first guard leaves the second reachable; the const-true
        // second guard then kills the else.
        let reach =
            reachability_of("IF $x { RETURN 0 } ELSE IF true { RETURN 1 } ELSE { RETURN 2 };");
        assert_eq!(
            reach.branches,
            vec![BranchReach::Reachable, BranchReach::Reachable]
        );
        assert!(reach.else_dead);
    }
}
