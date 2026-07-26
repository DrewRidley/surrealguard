//! `DEFINE FUNCTION` analysis.
//!
//! The definition's contract: the body must return what the `->` arrow
//! declares (2012). Parameters are bound with their declared kinds so the
//! body gets real analysis (the usual expression and statement checks run
//! inside it).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::contract::{term_kind, Contract, Position};
use crate::analyzer::facts::{eval, Bindings};
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

/// The kind a body that provably returns one constant returns, or `None` when
/// it does not have that shape.
///
/// The single `RETURN <const>;` body is the whole of it on purpose. A body with
/// several exits is several positions, and pointing one 2012 at the arrow for
/// the union of what they return is the shape this check already has; making it
/// point at the offending `RETURN` is compositional checking, which needs the
/// bidirectional walk this stage does not build.
fn returned_constant_kind(def: &ast::DefineFunction) -> Option<Kind> {
    let body = def.body.as_ref()?;
    let [statement] = body.statements.as_slice() else {
        return None;
    };
    let ast::Statement::Return(ret) = &statement.node else {
        return None;
    };
    term_kind(&eval(&ret.value.as_ref()?.node, Bindings::NONE))
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
            // `-> 'a' | 'b' { RETURN 'c'; }` compared the widened `string`,
            // which can never be shown to fall outside a string-literal union.
            // A body that provably returns one constant is checked as that
            // constant. Only the single-`RETURN` shape folds today: a body with
            // several exits is a set of positions rather than one value, and
            // splitting 2012 across them is the compositional-checking work,
            // not this.
            let actual =
                returned_constant_kind(stmt).unwrap_or_else(|| body_kind.clone());
            let contract = Contract::new(Position::FunctionReturn, declared.clone());
            if contract.decide(&actual).is_violation() {
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
                        contract.code(),
                        format!(
                            "`{}` declares `-> {}` but its body returns `{}`",
                            stmt.name.node,
                            crate::render_kind(&declared),
                            crate::render::render_offending(&actual, Some(&declared))
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
