//! `DEFINE FUNCTION` analysis.
//!
//! The definition's contract: the body must return what the `->` arrow
//! declares (2012). Parameters are bound with their declared kinds so the
//! body gets real analysis (the usual expression and statement checks run
//! inside it).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::expression::{ExpressionFact, ExpressionValueClass};

/// Analyzes a `DEFINE FUNCTION` body with its parameters bound to their
/// declared kinds (untyped params bind as `Any`) and returns the body's
/// response kind. Returns `None` when the definition has no body.
///
/// Running the body here is what surfaces the usual expression/statement
/// diagnostics inside it, and the returned kind is both what the `-> T`
/// contract (2012) is checked against and what an untyped function's callers
/// infer (persisted on [`crate::schema::FunctionDef::inferred_return`]).
pub(crate) fn infer_function_body_kind(
    ctx: &mut AnalysisContext<'_>,
    def: &ast::DefineFunction,
) -> Option<Kind> {
    let body = def.body.as_ref()?;
    let kind = ctx.with_child_env(|ctx| {
        // Engine-supplied session params (`$auth`, ...) are available in every
        // `fn::` body. Seeding here also covers the throwaway return-inference
        // path (`schema::infer_untyped_return`), which builds a fresh env; the
        // params bound below override the seed if they collide (they can't —
        // `$auth` and friends are protected names).
        ctx.seed_session_params();
        for (name, ty) in &def.params {
            let kind = ty
                .as_ref()
                .and_then(|ty| crate::schema::kind_from_type_expr(&ty.node, ctx.source_text()).kind)
                .unwrap_or(Kind::Any);
            let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), name.span);
            let mut fact = ExpressionFact::new(span, ExpressionValueClass::Variable);
            fact.kind = Some(kind);
            ctx.define_local(name.node.trim_start_matches('$').to_string(), fact);
        }
        crate::analyzer::flow::block::analyze_block(ctx, body)
    });
    Some(kind)
}

pub(crate) fn analyze_define_function(
    ctx: &mut AnalysisContext<'_>,
    stmt: &ast::DefineFunction,
) -> Kind {
    let Some(body_kind) = infer_function_body_kind(ctx, stmt) else {
        return Kind::None;
    };

    if let Some(return_ty) = &stmt.return_ty {
        if let Some(declared) =
            crate::schema::kind_from_type_expr(&return_ty.node, ctx.source_text()).kind
        {
            if body_kind != Kind::Any && !crate::kinds::kind_is_assignable_to(&body_kind, &declared)
            {
                let span = surrealguard_syntax::span::SourceSpan::new(
                    ctx.source().clone(),
                    return_ty.span,
                );
                let name_span = surrealguard_syntax::span::SourceSpan::new(
                    ctx.source().clone(),
                    stmt.name.span,
                );
                ctx.emit(
                    surrealguard_diagnostics::catalog::finding(
                        span,
                        2012,
                        format!(
                            "`{}` declares `-> {}` but its body returns `{}`",
                            stmt.name.node,
                            crate::render_kind(&declared),
                            crate::render_kind(&body_kind)
                        ),
                    )
                    .with_related(
                        name_span,
                        format!("`{}` is defined here", stmt.name.node),
                    ),
                );
            }
        }
    }

    Kind::None
}
