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

pub(crate) fn analyze_define_function(
    ctx: &mut AnalysisContext<'_>,
    stmt: &ast::DefineFunction,
) -> Kind {
    let Some(body) = &stmt.body else {
        return Kind::None;
    };

    let body_kind = ctx.with_child_env(|ctx| {
        for (name, ty) in &stmt.params {
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
                ctx.emit(surrealguard_diagnostics::catalog::finding(
                    span,
                    2012,
                    format!(
                        "`{}` declares `-> {declared}` but its body returns `{body_kind}`",
                        stmt.name.node
                    ),
                ));
            }
        }
    }

    Kind::None
}
