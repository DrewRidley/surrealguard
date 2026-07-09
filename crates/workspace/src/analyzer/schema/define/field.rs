//! `DEFINE FIELD` analysis.
//!
//! The definition's own contracts: a `DEFAULT` (or computed `VALUE`) must
//! inhabit the declared type (2001); an `ASSERT` is a condition with
//! `$value` in scope as the declared type (2005 when it can never be a
//! bool, plus the usual expression checking); and computed contexts should
//! not block or reach out (7012).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::expression::{ExpressionFact, ExpressionValueClass};

pub fn analyze_define_field(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineField) -> Kind {
    let declared = stmt
        .ty
        .as_ref()
        .and_then(|ty| crate::schema::kind_from_type_expr(&ty.node, ctx.source_text()).kind);

    for (clause, checks_type) in [(&stmt.default, true), (&stmt.value, true)] {
        let Some(expr) = clause else {
            continue;
        };
        let kind = with_value_bound(ctx, declared.clone(), |ctx| {
            let fact = crate::analyzer::expression::infer::infer_expression_fact(expr, ctx);
            crate::analyzer::expression::check::check_value_expression(ctx, expr);
            fact.kind
        });
        check_computed_calls(ctx, expr);
        if !checks_type {
            continue;
        }
        if let (Some(declared), Some(kind)) = (&declared, kind) {
            if kind != Kind::Any && !crate::semantic::kind_is_assignable_to(&kind, declared) {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), expr.span);
                ctx.emit(surrealguard_diagnostics::catalog::finding(
                    span,
                    2001,
                    format!(
                        "field `{}` expects `{declared}`, found `{kind}`",
                        idiom_text(&stmt.path.node)
                    ),
                ));
            }
        }
    }

    if let Some(assert) = &stmt.assert {
        let kind = with_value_bound(ctx, declared.clone(), |ctx| {
            let fact = crate::analyzer::expression::infer::infer_expression_fact(assert, ctx);
            crate::analyzer::expression::check::check_value_expression(ctx, assert);
            fact.kind
        });
        check_computed_calls(ctx, assert);
        if let Some(kind) = kind {
            if crate::analyzer::flow::if_else::definitely_not_bool(&kind) {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), assert.span);
                ctx.emit(surrealguard_diagnostics::catalog::finding(
                    span,
                    2005,
                    format!("ASSERT has type `{kind}`, expected `bool`"),
                ));
            }
        }
    }

    Kind::None
}

/// Runs `f` with `$value` (and `$input`) bound: `$value` carries the
/// declared type inside `ASSERT`/`VALUE`/`DEFAULT` bodies.
fn with_value_bound<T>(
    ctx: &mut AnalysisContext<'_>,
    declared: Option<Kind>,
    f: impl FnOnce(&mut AnalysisContext<'_>) -> T,
) -> T {
    ctx.with_child_env(|ctx| {
        let span = surrealguard_syntax::span::SourceSpan::new(
            ctx.source().clone(),
            surrealguard_syntax::span::ByteRange::new(0, 0).expect("empty range is ordered"),
        );
        let mut fact = ExpressionFact::new(span.clone(), ExpressionValueClass::Variable);
        fact.kind = declared;
        ctx.define_local("value".to_string(), fact);
        let mut input = ExpressionFact::new(span, ExpressionValueClass::Variable);
        input.kind = Some(Kind::Any);
        ctx.define_local("input".to_string(), input);
        f(ctx)
    })
}

/// Computed contexts run on every write; blocking or side-effecting calls
/// there are a footgun (7012).
fn check_computed_calls(ctx: &mut AnalysisContext<'_>, expr: &ast::Spanned<ast::Expr>) {
    match &expr.node {
        ast::Expr::Call(call) => {
            let path = call.path.node.as_str();
            if path.starts_with("http::") || path == "sleep::sleep" || path == "sleep" {
                let span = surrealguard_syntax::span::SourceSpan::new(
                    ctx.source().clone(),
                    call.path.span,
                );
                ctx.emit(surrealguard_diagnostics::catalog::finding(
                    span,
                    7012,
                    format!("`{path}` runs on every write from a computed field clause"),
                ));
            }
            for arg in &call.args {
                check_computed_calls(ctx, arg);
            }
        }
        ast::Expr::Binary { lhs, rhs, .. } => {
            check_computed_calls(ctx, lhs);
            check_computed_calls(ctx, rhs);
        }
        ast::Expr::Prefix { expr: inner, .. } | ast::Expr::Cast { expr: inner, .. } => {
            check_computed_calls(ctx, inner);
        }
        ast::Expr::Array(elements) => {
            for element in elements {
                check_computed_calls(ctx, element);
            }
        }
        ast::Expr::Object(fields) => {
            for (_, value) in fields {
                check_computed_calls(ctx, value);
            }
        }
        _ => {}
    }
}

fn idiom_text(idiom: &ast::Idiom) -> String {
    crate::analyzer::expression::infer::plain_field_segments(idiom)
        .map(|segments| segments.join("."))
        .unwrap_or_default()
}
