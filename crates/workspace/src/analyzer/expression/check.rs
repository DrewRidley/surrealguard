//! Expression invariant checking.
//!
//! The checking-side twin of [`super::infer`]: inference computes kinds
//! and never rejects; this walk reads the same expressions and emits
//! findings where an invariant is violated. Both run at the sites that
//! own the expressions.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;
use surrealguard_syntax::span::SourceSpan;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::expression::infer::{binary_result_kind, infer_expression_fact, is_numeric};

/// Recursively checks operator invariants in a value expression:
/// incompatible operands (2004) and negation of non-numerics (2014).
/// Unknown kinds are never violations.
pub(crate) fn check_value_expression(
    ctx: &mut AnalysisContext<'_>,
    expr: &ast::Spanned<ast::Expr>,
) {
    match &expr.node {
        ast::Expr::Binary { lhs, op, rhs } => {
            check_value_expression(ctx, lhs);
            check_value_expression(ctx, rhs);
            check_binary(ctx, expr, lhs, &op.node, rhs);
        }
        ast::Expr::Prefix { op, expr: inner } => {
            check_value_expression(ctx, inner);
            if matches!(op.node, ast::PrefixOp::Neg | ast::PrefixOp::Pos) {
                if let Some(kind) = known_kind(ctx, inner) {
                    if !is_numeric(&kind) {
                        emit(
                            ctx,
                            expr.span,
                            2004,
                            format!("incompatible operand for `-`: `{kind}`"),
                        );
                    }
                }
            }
        }
        ast::Expr::Array(elements) => {
            for element in elements {
                check_value_expression(ctx, element);
            }
            check_mixed_array(ctx, expr, elements);
        }
        ast::Expr::Object(fields) => {
            for (_, value) in fields {
                check_value_expression(ctx, value);
            }
        }
        ast::Expr::Call(call) => {
            for arg in &call.args {
                check_value_expression(ctx, arg);
            }
        }
        ast::Expr::Cast { expr: inner, .. } => check_value_expression(ctx, inner),
        _ => {}
    }
}

fn check_binary(
    ctx: &mut AnalysisContext<'_>,
    whole: &ast::Spanned<ast::Expr>,
    lhs: &ast::Spanned<ast::Expr>,
    op: &ast::BinaryOp,
    rhs: &ast::Spanned<ast::Expr>,
) {
    let (Some(left), Some(right)) = (known_kind(ctx, lhs), known_kind(ctx, rhs)) else {
        return;
    };

    use ast::BinaryOp as Op;
    // One contract, one code: the operands must make sense together for
    // the operator (2004). Whether SurrealDB throws (arithmetic) or
    // silently kind-orders (comparisons) is irrelevant — tolerated misuse
    // is still misuse; surfacing it is this tool's entire purpose.
    let violated = match op {
        Op::Add | Op::Sub | Op::Mul | Op::Div => binary_result_kind(op, &left, &right).is_none(),
        // NONE/NULL comparisons are the idiomatic existence checks;
        // numerics widen. Truthiness makes AND/OR legal on anything; ??
        // accepts anything by design.
        Op::Eq | Op::NotEq | Op::Lt | Op::LtEq | Op::Gt | Op::GtEq => !comparable(&left, &right),
        _ => false,
    };
    if violated {
        emit(
            ctx,
            whole.span,
            2004,
            format!(
                "incompatible operands for `{}`: `{left}` and `{right}`",
                op_text(op)
            ),
        );
    }
}

fn comparable(left: &Kind, right: &Kind) -> bool {
    if left == right
        || (is_numeric(left) && is_numeric(right))
        || matches!(left, Kind::None | Kind::Null)
        || matches!(right, Kind::None | Kind::Null)
    {
        return true;
    }
    // Records compare across tables (id ordering); literals compare with
    // their base kind; unions compare if any variant does.
    match (left, right) {
        (Kind::Record(_), Kind::Record(_)) => true,
        (Kind::Either(variants), other) | (other, Kind::Either(variants)) => {
            variants.iter().any(|variant| comparable(variant, other))
        }
        _ => {
            let left = crate::semantic::literal_base_kind(left);
            let right = crate::semantic::literal_base_kind(right);
            match (left, right) {
                (None, None) => false,
                (l, r) => {
                    let l = l.unwrap_or(Kind::Any);
                    let r = r.unwrap_or(Kind::Any);
                    l == r || matches!(l, Kind::Any) || matches!(r, Kind::Any)
                }
            }
        }
    }
}

/// An array literal whose elements have several kinds usually wants a
/// review (7003) — heterogeneous arrays are legal but rarely intended.
fn check_mixed_array(
    ctx: &mut AnalysisContext<'_>,
    whole: &ast::Spanned<ast::Expr>,
    elements: &[ast::Spanned<ast::Expr>],
) {
    let mut kinds: Vec<Kind> = Vec::new();
    for element in elements {
        let Some(kind) = known_kind(ctx, element) else {
            return;
        };
        let base = crate::semantic::literal_base_kind(&kind).unwrap_or(kind);
        if !kinds.contains(&base) {
            kinds.push(base);
        }
    }
    if kinds.len() > 1 {
        emit(
            ctx,
            whole.span,
            7003,
            format!(
                "array literal mixes kinds: {}",
                kinds
                    .iter()
                    .map(|k| format!("`{k}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    }
}

/// A kind that is actually informative: `Any` never violates anything.
fn known_kind(ctx: &mut AnalysisContext<'_>, expr: &ast::Spanned<ast::Expr>) -> Option<Kind> {
    let kind = infer_expression_fact(expr, ctx).kind?;
    (kind != Kind::Any).then_some(kind)
}

fn op_text(op: &ast::BinaryOp) -> &'static str {
    use ast::BinaryOp as Op;
    match op {
        Op::Add => "+",
        Op::Sub => "-",
        Op::Mul => "*",
        Op::Div => "/",
        Op::Eq => "=",
        Op::NotEq => "!=",
        Op::Lt => "<",
        Op::LtEq => "<=",
        Op::Gt => ">",
        Op::GtEq => ">=",
        Op::And => "AND",
        Op::Or => "OR",
        Op::NullCoalesce => "??",
        _ => "?",
    }
}

fn emit(
    ctx: &mut AnalysisContext<'_>,
    span: surrealguard_syntax::span::ByteRange,
    code: u16,
    message: String,
) {
    let span = SourceSpan::new(ctx.source().clone(), span);
    ctx.emit(surrealguard_diagnostics::catalog::finding(
        span, code, message,
    ));
}
