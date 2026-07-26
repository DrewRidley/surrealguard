//! Branch reachability: which arms of an `IF`/`ELSE` chain can run.
//!
//! The folding this rests on lives in [`crate::analyzer::facts::term`] — one
//! folder for the whole analyzer, rather than the four that used to disagree
//! about what a constant is. What stays here is the *policy*: how a folded
//! guard turns into a reachability verdict, which is a statement about
//! control flow rather than about expressions.
//!
//! The folder's contract still governs everything below: it folds only what it
//! can prove and answers `None` the instant an operand is runtime-dependent,
//! so greying a branch never rests on a guess.

use surrealguard_syntax::ast;

use crate::analyzer::facts::term::{fold, fold_bool, Bindings};

pub use crate::analyzer::facts::term::ConstValue;

/// Folds `expr` to a [`ConstValue`], or `None` when it is not provably a
/// constant.
///
/// A thin reading of [`crate::analyzer::facts::eval`]: "the term is a
/// constant" is the only part of a denotation this module's callers want.
pub fn const_eval(expr: &ast::Expr) -> Option<ConstValue> {
    fold(expr, Bindings::NONE)
}

/// Folds `expr` to a boolean constant, or `None` when it does not provably
/// reduce to a `bool`. This is what guard reachability tests.
pub fn const_eval_bool(expr: &ast::Expr) -> Option<bool> {
    fold_bool(expr, Bindings::NONE)
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
/// Folds only constants (see [`const_eval_bool`]): a guard mentioning any
/// param, field, function, or idiom is unknown, so its branch is kept.
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
        match const_eval_bool(&branch.condition.node) {
            Some(false) => branches.push(BranchReach::DeadFalse),
            Some(true) => {
                branches.push(BranchReach::Reachable);
                taken = true;
            }
            None => branches.push(BranchReach::Reachable),
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
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    /// Folds the first expression parsed from `source` (as a `RETURN <expr>`).
    fn fold(source: &str) -> Option<ConstValue> {
        let query = format!("RETURN {source};");
        let parsed = parse_source(SourceId::new("const:test"), query.as_str()).expect("query parses");
        let ast::Statement::Return(stmt) =
            surrealguard_syntax::lower::lower_first_statement(&parsed, "ReturnStatement")
                .expect("no ReturnStatement node")
                .node
        else {
            panic!("expected return statement");
        };
        const_eval(&stmt.value.expect("return has a value").node)
    }

    #[test]
    fn folds_literals() {
        assert_eq!(fold("1"), Some(ConstValue::Int(1)));
        assert_eq!(fold("'a'"), Some(ConstValue::Str("a".into())));
        assert_eq!(fold("true"), Some(ConstValue::Bool(true)));
        assert_eq!(fold("NONE"), Some(ConstValue::None));
        assert_eq!(fold("-3"), Some(ConstValue::Int(-3)));
    }

    #[test]
    fn folds_comparisons() {
        assert_eq!(fold("1 == 1"), Some(ConstValue::Bool(true)));
        assert_eq!(fold("2 > 3"), Some(ConstValue::Bool(false)));
        assert_eq!(fold("2 < 3"), Some(ConstValue::Bool(true)));
        assert_eq!(fold("'a' == 'a'"), Some(ConstValue::Bool(true)));
        assert_eq!(fold("'a' != 'b'"), Some(ConstValue::Bool(true)));
        assert_eq!(fold("2 >= 2"), Some(ConstValue::Bool(true)));
    }

    #[test]
    fn folds_booleans_with_short_circuit() {
        assert_eq!(fold("true AND false"), Some(ConstValue::Bool(false)));
        assert_eq!(fold("true OR false"), Some(ConstValue::Bool(true)));
        assert_eq!(fold("!false"), Some(ConstValue::Bool(true)));
        // A provable side decides the result even when the other is unknown.
        assert_eq!(fold("false AND $x"), Some(ConstValue::Bool(false)));
        assert_eq!(fold("true OR $x"), Some(ConstValue::Bool(true)));
    }

    #[test]
    fn folds_arithmetic() {
        assert_eq!(fold("2 + 2"), Some(ConstValue::Int(4)));
        assert_eq!(fold("2 + 2 == 4"), Some(ConstValue::Bool(true)));
        assert_eq!(fold("3 * 4 - 2"), Some(ConstValue::Int(10)));
    }

    #[test]
    fn non_constant_operands_are_unknown() {
        assert_eq!(fold("$x == 1"), None);
        assert_eq!(fold("field.a == 1"), None);
        assert_eq!(fold("rand::bool()"), None);
        assert_eq!(fold("$x"), None);
        // A partial fold with one unknown side is still unknown (no guess).
        assert_eq!(fold("1 == 1 AND $x"), None);
    }

    #[test]
    fn const_eval_bool_only_returns_bool_constants() {
        assert_eq!(const_eval_bool_of("1 == 1"), Some(true));
        assert_eq!(const_eval_bool_of("2 > 3"), Some(false));
        // A numeric constant is not a bool.
        assert_eq!(const_eval_bool_of("1"), None);
        assert_eq!(const_eval_bool_of("$x"), None);
    }

    fn const_eval_bool_of(source: &str) -> Option<bool> {
        let query = format!("RETURN {source};");
        let parsed = parse_source(SourceId::new("const:test"), query.as_str()).expect("query parses");
        let ast::Statement::Return(stmt) =
            surrealguard_syntax::lower::lower_first_statement(&parsed, "ReturnStatement")
                .expect("no ReturnStatement node")
                .node
        else {
            panic!("expected return statement");
        };
        const_eval_bool(&stmt.value.expect("return has a value").node)
    }

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
        let reach = reachability_of("IF $x { RETURN 0 } ELSE IF true { RETURN 1 } ELSE { RETURN 2 };");
        assert_eq!(
            reach.branches,
            vec![BranchReach::Reachable, BranchReach::Reachable]
        );
        assert!(reach.else_dead);
    }
}
