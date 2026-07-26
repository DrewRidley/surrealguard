//! `Term` — the canonical denotation of a *value*, and the one constant
//! folder.
//!
//! Four folders shipped here, with four different literal coverages:
//! `const_eval.rs` folded arithmetic and comparisons but not membership;
//! `schema/define/field.rs::fold_const` folded membership and `$value` but not
//! arithmetic; `select.rs::literal_limit` matched exactly one `Int` literal;
//! `pipeline.rs`'s `max_len` matched the same literal a second time. A
//! `LIMIT (1)` was not a limit, and a `LIMIT 1 + 0` was not either.
//!
//! They are one question — *what does this expression denote?* — and [`eval`]
//! answers it once. The answer is a [`Term`]: a location, a constant, a
//! discriminant of a location, or nothing this layer can prove.
//!
//! The contract is inherited verbatim from `const_eval`: **soundness over
//! completeness**. Folding stops at the first operand that is
//! runtime-dependent and yields [`Term::Opaque`] rather than a guess, because
//! a caller that greys a branch or drops a type contribution on the strength
//! of a fold must never act on uncertainty.

use surrealdb_types::{Kind, KindLiteral};
use surrealguard_syntax::ast;

use super::place::{place_of, Place};

/// A value the folder can reason about.
///
/// Anything the folder cannot prove is a constant of one of these shapes is
/// [`Term::Opaque`], never a `ConstValue` — it bails rather than guess.
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

impl ConstValue {
    /// The singleton [`Kind`] this value inhabits, for the `f = <lit>`
    /// refinement.
    ///
    /// `NONE`/`NULL` have no literal kind — they are sentinels, handled by
    /// their own atoms — and neither do the values whose literal kind cannot
    /// be written (datetime, uuid, regex).
    pub(crate) fn singleton_kind(&self) -> Option<Kind> {
        match self {
            ConstValue::Int(value) => Some(Kind::Literal(KindLiteral::Integer(*value))),
            ConstValue::Float(value) => Some(Kind::Literal(KindLiteral::Float(*value))),
            ConstValue::Str(value) => Some(Kind::Literal(KindLiteral::String(value.clone()))),
            ConstValue::Bool(value) => Some(Kind::Literal(KindLiteral::Bool(*value))),
            ConstValue::None | ConstValue::Null => None,
        }
    }

    /// The string this value holds, when it is one.
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            ConstValue::Str(value) => Some(value),
            _ => None,
        }
    }

    /// The boolean this value is, when it provably is one.
    pub(crate) fn as_bool(&self) -> Option<bool> {
        match self {
            ConstValue::Bool(value) => Some(*value),
            _ => None,
        }
    }
}

/// What an expression denotes, as far as this layer can prove.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Term {
    /// A refinable location.
    Place(Place),
    /// A statically known value.
    Const(ConstValue),
    /// A projection *of* a place that discriminates it. This is how
    /// `type::table($x)` stops being a special case: it is a discriminant term
    /// over the place `$x`, and a second spelling of the same fact costs one
    /// arm here rather than a new recognizer in every consumer.
    Discriminant {
        /// The place being discriminated.
        of: Place,
        /// Which projection of it.
        kind: DiscriminantKind,
    },
    /// Anything else: a call, a subquery, an arithmetic result over a
    /// non-constant operand.
    Opaque,
}

/// Which projection of a place a discriminant term takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiscriminantKind {
    /// `type::table(x)` — the table name of a record link.
    RecordTable,
}

/// The bindings [`eval`] may resolve.
///
/// `$value` is the only one today: a `DEFINE FIELD … ASSERT` is evaluated with
/// the folded `DEFAULT` bound to it. A `LET`-bound param is deliberately not
/// resolved — that would change which expressions fold, and the folder's
/// callers grey code on the result.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Bindings<'a> {
    value: Option<&'a ConstValue>,
}

impl<'a> Bindings<'a> {
    /// No bindings: the purely syntactic folder.
    pub(crate) const NONE: Bindings<'static> = Bindings { value: None };

    /// `$value` bound to a constant.
    pub(crate) fn value(value: &'a ConstValue) -> Self {
        Self { value: Some(value) }
    }
}

/// What `expr` denotes under `bindings`.
///
/// Folds literals; unary `-`/`+`/`!`; the comparison operators; boolean
/// `AND`/`OR` (with short-circuit, so a single provable side can decide the
/// result); `+`/`-`/`*` over numeric constants; and `IN`/`INSIDE`/`CONTAINS`
/// over an array literal of constants. Division is intentionally omitted:
/// SurrealDB's exact rounding for `/` is subtle enough that folding it risks
/// disagreeing with runtime, which would make a fold-driven grey unsound.
pub(crate) fn eval(expr: &ast::Expr, bindings: Bindings<'_>) -> Term {
    // `$value` shadows the place it would otherwise name: where it is bound,
    // it *is* the constant.
    if let (ast::Expr::Param(name), Some(value)) = (expr, bindings.value) {
        if name == "value" {
            return Term::Const(value.clone());
        }
    }
    if let Some(place) = place_of(expr) {
        return Term::Place(place);
    }
    match expr {
        ast::Expr::Literal(literal) => {
            literal_const(literal).map_or(Term::Opaque, Term::Const)
        }
        ast::Expr::Prefix { op, expr } => prefix_term(&op.node, &expr.node, bindings),
        ast::Expr::Binary { lhs, op, rhs } => {
            binary_term(&op.node, &lhs.node, &rhs.node, bindings)
        }
        ast::Expr::Call(call) => discriminant_term(call),
        // A `Statement::Expr` subquery is a parenthesized expression that the
        // lowering did not unwrap; anything else really is a statement.
        ast::Expr::Subquery(statement) => match &statement.node {
            ast::Statement::Expr(inner) => eval(&inner.node, bindings),
            _ => Term::Opaque,
        },
        _ => Term::Opaque,
    }
}

/// The constant `expr` folds to, or `None` when it is not provably one.
pub(crate) fn fold(expr: &ast::Expr, bindings: Bindings<'_>) -> Option<ConstValue> {
    match eval(expr, bindings) {
        Term::Const(value) => Some(value),
        _ => None,
    }
}

/// The boolean `expr` folds to, or `None` when it does not provably reduce to
/// one. This is what guard reachability tests.
pub(crate) fn fold_bool(expr: &ast::Expr, bindings: Bindings<'_>) -> Option<bool> {
    fold(expr, bindings)?.as_bool()
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

/// `type::table(<place>)` — the one discriminant spelling recognized today.
fn discriminant_term(call: &ast::Call) -> Term {
    if call.path.node != "type::table" {
        return Term::Opaque;
    }
    let [arg] = call.args.as_slice() else {
        return Term::Opaque;
    };
    place_of(&arg.node).map_or(Term::Opaque, |of| Term::Discriminant {
        of,
        kind: DiscriminantKind::RecordTable,
    })
}

fn prefix_term(op: &ast::PrefixOp, expr: &ast::Expr, bindings: Bindings<'_>) -> Term {
    let folded = match op {
        ast::PrefixOp::Neg => match fold(expr, bindings) {
            // Checked: an overflowing negation is not a constant we can
            // prove. `fold_const` negated unchecked, which panics in a debug
            // build; the grammar makes the input hard to write (a written
            // `-1` lexes as one signed literal, and `-(…)` does not parse),
            // which is why it was never hit rather than why it was safe.
            Some(ConstValue::Int(value)) => value.checked_neg().map(ConstValue::Int),
            Some(ConstValue::Float(value)) => Some(ConstValue::Float(-value)),
            _ => None,
        },
        ast::PrefixOp::Pos => fold(expr, bindings),
        ast::PrefixOp::Not => fold_bool(expr, bindings).map(|value| ConstValue::Bool(!value)),
        ast::PrefixOp::Other(_) => None,
    };
    folded.map_or(Term::Opaque, Term::Const)
}

fn binary_term(
    op: &ast::BinaryOp,
    lhs: &ast::Expr,
    rhs: &ast::Expr,
    bindings: Bindings<'_>,
) -> Term {
    use ast::BinaryOp;
    let folded = match op {
        // `AND`/`OR` short-circuit: one provable side can decide the whole even
        // when the other cannot fold (`false AND $x` is provably false).
        BinaryOp::And => {
            let left = fold_bool(lhs, bindings);
            let right = fold_bool(rhs, bindings);
            if left == Some(false) || right == Some(false) {
                Some(ConstValue::Bool(false))
            } else if left == Some(true) && right == Some(true) {
                Some(ConstValue::Bool(true))
            } else {
                None
            }
        }
        BinaryOp::Or => {
            let left = fold_bool(lhs, bindings);
            let right = fold_bool(rhs, bindings);
            if left == Some(true) || right == Some(true) {
                Some(ConstValue::Bool(true))
            } else if left == Some(false) && right == Some(false) {
                Some(ConstValue::Bool(false))
            } else {
                None
            }
        }
        BinaryOp::Eq => pair(lhs, rhs, bindings)
            .and_then(|(a, b)| const_eq(&a, &b))
            .map(ConstValue::Bool),
        BinaryOp::NotEq => pair(lhs, rhs, bindings)
            .and_then(|(a, b)| const_eq(&a, &b))
            .map(|equal| ConstValue::Bool(!equal)),
        BinaryOp::Lt => const_order(lhs, rhs, bindings)
            .map(|o| ConstValue::Bool(o == std::cmp::Ordering::Less)),
        BinaryOp::LtEq => const_order(lhs, rhs, bindings)
            .map(|o| ConstValue::Bool(o != std::cmp::Ordering::Greater)),
        BinaryOp::Gt => const_order(lhs, rhs, bindings)
            .map(|o| ConstValue::Bool(o == std::cmp::Ordering::Greater)),
        BinaryOp::GtEq => const_order(lhs, rhs, bindings)
            .map(|o| ConstValue::Bool(o != std::cmp::Ordering::Less)),
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul => arithmetic(op, lhs, rhs, bindings),
        BinaryOp::Other(name)
            if matches!(name.to_ascii_uppercase().as_str(), "IN" | "INSIDE") =>
        {
            membership(lhs, rhs, bindings).map(ConstValue::Bool)
        }
        BinaryOp::Other(name) if name.eq_ignore_ascii_case("contains") => {
            membership(rhs, lhs, bindings).map(ConstValue::Bool)
        }
        _ => None,
    };
    folded.map_or(Term::Opaque, Term::Const)
}

/// Both operands as constants, or `None` when either is not one.
fn pair(
    lhs: &ast::Expr,
    rhs: &ast::Expr,
    bindings: Bindings<'_>,
) -> Option<(ConstValue, ConstValue)> {
    Some((fold(lhs, bindings)?, fold(rhs, bindings)?))
}

/// Simple numeric arithmetic. Integer operations are checked and bail on
/// overflow.
fn arithmetic(
    op: &ast::BinaryOp,
    lhs: &ast::Expr,
    rhs: &ast::Expr,
    bindings: Bindings<'_>,
) -> Option<ConstValue> {
    use ast::BinaryOp;
    let (a, b) = pair(lhs, rhs, bindings)?;
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

/// `<element> IN [a, b, …]`: `Some(true)` when the element equals a folded
/// member, `Some(false)` when every member folds and none match, and `None`
/// when a member cannot fold — a non-match can't be proven then.
fn membership(element: &ast::Expr, collection: &ast::Expr, bindings: Bindings<'_>) -> Option<bool> {
    let element = fold(element, bindings)?;
    let ast::Expr::Array(members) = collection else {
        return None;
    };
    let mut all_folded = true;
    for member in members {
        match fold(&member.node, bindings) {
            Some(member) => {
                if const_eq(&element, &member) == Some(true) {
                    return Some(true);
                }
            }
            None => all_folded = false,
        }
    }
    all_folded.then_some(false)
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
fn const_order(
    lhs: &ast::Expr,
    rhs: &ast::Expr,
    bindings: Bindings<'_>,
) -> Option<std::cmp::Ordering> {
    let (a, b) = pair(lhs, rhs, bindings)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    /// The term of the expression in `RETURN <source>;`.
    fn term(source: &str, bindings: Bindings<'_>) -> Term {
        let query = format!("RETURN {source};");
        let parsed = parse_source(SourceId::new("term:test"), query.as_str()).expect("parses");
        let statements = surrealguard_syntax::lower::lower_statements(&parsed);
        let ast::Statement::Return(stmt) = &statements.first().expect("one statement").node else {
            panic!("expected a RETURN");
        };
        eval(&stmt.value.as_ref().expect("a value").node, bindings)
    }

    fn folded(source: &str) -> Option<ConstValue> {
        match term(source, Bindings::NONE) {
            Term::Const(value) => Some(value),
            _ => None,
        }
    }

    #[test]
    fn a_place_denotes_itself_and_a_literal_denotes_its_value() {
        assert_eq!(term("$x", Bindings::NONE), Term::Place(Place::param("x")));
        assert_eq!(folded("'a'"), Some(ConstValue::Str("a".into())));
        assert_eq!(folded("NONE"), Some(ConstValue::None));
        assert_eq!(folded("NULL"), Some(ConstValue::Null));
    }

    #[test]
    fn the_one_folder_covers_what_the_four_covered_between_them() {
        // Arithmetic came from `const_eval`; membership from `field.rs`. Each
        // was missing from the other, so `LIMIT 1 + 0` and
        // `ASSERT $value > 1 + 0` both went unfolded depending on which folder
        // happened to be looking.
        assert_eq!(folded("1 + 0"), Some(ConstValue::Int(1)));
        assert_eq!(folded("2 * 3 - 1"), Some(ConstValue::Int(5)));
        assert_eq!(folded("'a' IN ['a', 'b']"), Some(ConstValue::Bool(true)));
        assert_eq!(folded("'c' IN ['a', 'b']"), Some(ConstValue::Bool(false)));
        assert_eq!(folded("['a', 'b'] CONTAINS 'a'"), Some(ConstValue::Bool(true)));
        assert_eq!(folded("1 = 1"), Some(ConstValue::Bool(true)));
        assert_eq!(folded("!(1 = 2)"), Some(ConstValue::Bool(true)));
    }

    #[test]
    fn a_runtime_operand_stops_the_fold_rather_than_guessing_it() {
        assert_eq!(folded("$x + 1"), None);
        assert_eq!(folded("fn::f() = 1"), None);
        // A member that cannot fold makes a non-match unprovable.
        assert_eq!(folded("'c' IN ['a', $x]"), None);
        // …but a match is still a match.
        assert_eq!(folded("'a' IN ['a', $x]"), Some(ConstValue::Bool(true)));
        // Short-circuit survives the merge.
        assert_eq!(folded("false AND $x"), Some(ConstValue::Bool(false)));
        assert_eq!(folded("true OR $x"), Some(ConstValue::Bool(true)));
    }

    #[test]
    fn value_is_a_constant_where_it_is_bound_and_a_place_where_it_is_not() {
        let bound = ConstValue::Int(5);
        assert_eq!(
            term("$value > 3", Bindings::value(&bound)),
            Term::Const(ConstValue::Bool(true))
        );
        assert_eq!(
            term("$value IN [1, 5]", Bindings::value(&bound)),
            Term::Const(ConstValue::Bool(true))
        );
        assert_eq!(
            term("$value", Bindings::NONE),
            Term::Place(Place::param("value"))
        );
    }

    #[test]
    fn a_type_table_call_is_a_discriminant_over_the_place_it_projects() {
        assert_eq!(
            term("type::table($x)", Bindings::NONE),
            Term::Discriminant {
                of: Place::param("x"),
                kind: DiscriminantKind::RecordTable,
            }
        );
        // Not a place inside, not a discriminant.
        assert_eq!(term("type::table(fn::f())", Bindings::NONE), Term::Opaque);
        assert_eq!(term("string::len('a')", Bindings::NONE), Term::Opaque);
    }
}
