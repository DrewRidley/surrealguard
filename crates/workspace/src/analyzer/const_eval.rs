//! A small, **pure** constant folder over the expression AST, plus the
//! branch-reachability analysis it powers.
//!
//! The folder answers one question: *does this expression provably reduce to a
//! single constant value?* It sees only the syntax — no context, no schema, no
//! diagnostics — so it can be reused anywhere a purely-syntactic constant is
//! useful (guard folding, `DEFINE FIELD` DEFAULT-vs-ASSERT, …).
//!
//! Its contract is **soundness over completeness**: it folds only literals and
//! operations over already-folded operands, and returns `None` ("unknown") the
//! instant an operand is anything runtime-dependent — a param, a field idiom, a
//! function call. It never guesses. Callers that grey code or drop a type
//! contribution on the strength of a fold therefore never act on uncertainty.

use surrealguard_syntax::ast;

/// A value the constant folder can reason about. Anything the folder cannot
/// prove is a constant of one of these shapes is represented as `None` by the
/// folding functions, never as a `ConstValue` — the folder bails rather than
/// guess.
#[derive(Clone, Debug, PartialEq)]
pub enum ConstValue {
    /// An integer constant.
    Int(i64),
    /// A floating-point constant.
    Float(f64),
    /// A string constant.
    Str(String),
    /// A boolean constant.
    Bool(bool),
    /// The `NONE` sentinel.
    None,
    /// The `NULL` sentinel.
    Null,
}

/// Folds `expr` to a [`ConstValue`], or `None` when it is not provably a
/// constant. Pure: no context, no side effects.
///
/// Folds literals; unary `-`/`+`/`!`; the comparison operators
/// (`==`,`!=`,`<`,`<=`,`>`,`>=`); boolean `AND`/`OR` (with short-circuit, so a
/// single provable side can decide the result); and simple `+`/`-`/`*`
/// arithmetic over numeric constants. Any non-constant operand collapses the
/// whole expression to `None`.
pub fn const_eval(expr: &ast::Expr) -> Option<ConstValue> {
    match expr {
        ast::Expr::Literal(literal) => literal_const(literal),
        ast::Expr::Prefix { op, expr } => prefix_const(&op.node, &expr.node),
        ast::Expr::Binary { lhs, op, rhs } => binary_const(&op.node, &lhs.node, &rhs.node),
        // `(1 == 1)` needs no arm here: grouping parentheses lower to the
        // inner expression, so the folder sees the `Binary` directly. A
        // surviving `Expr::Subquery` wraps a real statement, which is not a
        // constant.
        _ => None,
    }
}

/// Convenience: folds `expr` to a boolean constant, or `None` when it does not
/// provably reduce to a `bool`. This is what guard-reachability tests.
pub fn const_eval_bool(expr: &ast::Expr) -> Option<bool> {
    match const_eval(expr)? {
        ConstValue::Bool(value) => Some(value),
        _ => None,
    }
}

fn literal_const(literal: &ast::Literal) -> Option<ConstValue> {
    match literal {
        ast::Literal::Int(value) => Some(ConstValue::Int(*value)),
        ast::Literal::Float(value) => Some(ConstValue::Float(*value)),
        ast::Literal::String(value) => Some(ConstValue::Str(value.clone())),
        ast::Literal::Bool(value) => Some(ConstValue::Bool(*value)),
        ast::Literal::None => Some(ConstValue::None),
        ast::Literal::Null => Some(ConstValue::Null),
        _ => None,
    }
}

fn prefix_const(op: &ast::PrefixOp, expr: &ast::Expr) -> Option<ConstValue> {
    match op {
        ast::PrefixOp::Neg => match const_eval(expr)? {
            ConstValue::Int(value) => Some(ConstValue::Int(value.checked_neg()?)),
            ConstValue::Float(value) => Some(ConstValue::Float(-value)),
            _ => None,
        },
        ast::PrefixOp::Pos => const_eval(expr),
        ast::PrefixOp::Not => match const_eval(expr)? {
            ConstValue::Bool(value) => Some(ConstValue::Bool(!value)),
            _ => None,
        },
        ast::PrefixOp::Other(_) => None,
    }
}

fn binary_const(op: &ast::BinaryOp, lhs: &ast::Expr, rhs: &ast::Expr) -> Option<ConstValue> {
    use ast::BinaryOp;
    match op {
        // `AND`/`OR` short-circuit: one provable side can decide the whole even
        // when the other cannot fold (`false AND $x` is provably false).
        BinaryOp::And => {
            let left = const_eval_bool(lhs);
            let right = const_eval_bool(rhs);
            if left == Some(false) || right == Some(false) {
                Some(ConstValue::Bool(false))
            } else if left == Some(true) && right == Some(true) {
                Some(ConstValue::Bool(true))
            } else {
                None
            }
        }
        BinaryOp::Or => {
            let left = const_eval_bool(lhs);
            let right = const_eval_bool(rhs);
            if left == Some(true) || right == Some(true) {
                Some(ConstValue::Bool(true))
            } else if left == Some(false) && right == Some(false) {
                Some(ConstValue::Bool(false))
            } else {
                None
            }
        }
        BinaryOp::Eq => const_eq(&const_eval(lhs)?, &const_eval(rhs)?).map(ConstValue::Bool),
        BinaryOp::NotEq => {
            const_eq(&const_eval(lhs)?, &const_eval(rhs)?).map(|equal| ConstValue::Bool(!equal))
        }
        BinaryOp::Lt => {
            const_order(lhs, rhs).map(|o| ConstValue::Bool(o == std::cmp::Ordering::Less))
        }
        BinaryOp::LtEq => {
            const_order(lhs, rhs).map(|o| ConstValue::Bool(o != std::cmp::Ordering::Greater))
        }
        BinaryOp::Gt => {
            const_order(lhs, rhs).map(|o| ConstValue::Bool(o == std::cmp::Ordering::Greater))
        }
        BinaryOp::GtEq => {
            const_order(lhs, rhs).map(|o| ConstValue::Bool(o != std::cmp::Ordering::Less))
        }
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul => arithmetic(op, lhs, rhs),
        _ => None,
    }
}

/// Simple numeric arithmetic. Division is intentionally omitted: SurrealDB's
/// exact numeric/rounding semantics for `/` are subtle enough that folding it
/// risks disagreeing with runtime, which would make a fold-driven grey unsound.
/// Integer operations use checked arithmetic and bail on overflow.
fn arithmetic(op: &ast::BinaryOp, lhs: &ast::Expr, rhs: &ast::Expr) -> Option<ConstValue> {
    use ast::BinaryOp;
    let a = const_eval(lhs)?;
    let b = const_eval(rhs)?;
    match (&a, &b) {
        (ConstValue::Int(x), ConstValue::Int(y)) => {
            let value = match op {
                BinaryOp::Add => x.checked_add(*y)?,
                BinaryOp::Sub => x.checked_sub(*y)?,
                BinaryOp::Mul => x.checked_mul(*y)?,
                _ => return None,
            };
            Some(ConstValue::Int(value))
        }
        _ => {
            let x = const_as_f64(&a)?;
            let y = const_as_f64(&b)?;
            let value = match op {
                BinaryOp::Add => x + y,
                BinaryOp::Sub => x - y,
                BinaryOp::Mul => x * y,
                _ => return None,
            };
            Some(ConstValue::Float(value))
        }
    }
}

/// Constant equality. `None` when the two values are not comparable under a
/// shape the folder proves (bail rather than assume unequal). Numeric kinds
/// compare across `int`/`float`.
fn const_eq(a: &ConstValue, b: &ConstValue) -> Option<bool> {
    match (a, b) {
        (ConstValue::Int(x), ConstValue::Int(y)) => Some(x == y),
        (ConstValue::Float(x), ConstValue::Float(y)) => Some(x == y),
        (ConstValue::Int(x), ConstValue::Float(y)) | (ConstValue::Float(y), ConstValue::Int(x)) => {
            Some(*x as f64 == *y)
        }
        (ConstValue::Str(x), ConstValue::Str(y)) => Some(x == y),
        (ConstValue::Bool(x), ConstValue::Bool(y)) => Some(x == y),
        (ConstValue::None, ConstValue::None) => Some(true),
        (ConstValue::Null, ConstValue::Null) => Some(true),
        // Distinct sentinels / distinct scalar kinds never compare equal.
        (ConstValue::None | ConstValue::Null, _) | (_, ConstValue::None | ConstValue::Null) => {
            Some(false)
        }
        _ => None,
    }
}

/// Orders two operands numerically (or lexically for strings). `None` for any
/// pairing the folder cannot compare.
fn const_order(lhs: &ast::Expr, rhs: &ast::Expr) -> Option<std::cmp::Ordering> {
    let a = const_eval(lhs)?;
    let b = const_eval(rhs)?;
    match (&a, &b) {
        (ConstValue::Int(x), ConstValue::Int(y)) => Some(x.cmp(y)),
        (ConstValue::Str(x), ConstValue::Str(y)) => Some(x.cmp(y)),
        _ => {
            let x = const_as_f64(&a)?;
            let y = const_as_f64(&b)?;
            x.partial_cmp(&y)
        }
    }
}

fn const_as_f64(value: &ConstValue) -> Option<f64> {
    match value {
        ConstValue::Int(v) => Some(*v as f64),
        ConstValue::Float(v) => Some(*v),
        _ => None,
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
