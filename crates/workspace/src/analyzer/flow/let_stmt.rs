//! `LET` statement analysis.
//!
//! Evaluates the bound value and defines it on the statement environment so
//! later statements in the same scope resolve `$name` to its inferred kind.
//! The statement itself produces no value.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_let(ctx: &mut AnalysisContext<'_>, stmt: &ast::LetStmt) -> Kind {
    if ctx.env().would_shadow(&stmt.name.node) {
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), stmt.name.span);
        ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                span,
                7002,
                format!(
                    "`${}` is re-bound inside this block; the outer `${}` is unchanged",
                    stmt.name.node, stmt.name.node
                ),
            )
            .with_help(
                "a block introduces a new scope, so this LET does not affect the outer binding",
            )
            .with_help("silence with `W7002 = \"allow\"`"),
        );
    }
    // SurrealDB rejects assignment to a parameter it binds itself. The set is
    // `context_params`' one table, not a copy of it: the copy that used to
    // stand here was missing `$scope` and `$self`, both of which the engine
    // binds and this check silently allowed.
    if crate::context_params::is_engine_param(&stmt.name.node) {
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), stmt.name.span);
        ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                span,
                6007,
                format!(
                    "`${}` is a protected parameter and can't be assigned",
                    stmt.name.node
                ),
            )
            .with_help(
                "protected parameters (`$this`, `$parent`, `$value`, ...) are bound by the engine",
            ),
        );
    }

    let fact = crate::analyzer::expression::expr_fact(ctx, &stmt.value);
    // 6002: this LET rebinds a `DEFINE PARAM` to an incompatible kind. A
    // DEFINE PARAM's kind is its `VALUE` expression's inferred kind, recorded
    // on the env in source order — so this fires only when the param was
    // defined *before* this LET, both kinds are known, and neither fits the
    // other (a provable clash such as `DEFINE PARAM $min_age VALUE 18` then
    // `LET $min_age = 'x'`). Compatible rebinds (`int` over `number`) and any
    // `any`/unknown side never fire.
    if let Some(param_kind) = ctx
        .env()
        .param_default_fact(&stmt.name.node)
        .and_then(|default| default.kind.clone())
    {
        if let Some(let_kind) = &fact.kind {
            if shadows_with_different_kind(let_kind, &param_kind) {
                let span = surrealguard_syntax::span::SourceSpan::new(
                    ctx.source().clone(),
                    stmt.name.span,
                );
                ctx.emit(
                    surrealguard_diagnostics::catalog::finding(
                        span,
                        6002,
                        format!(
                            "`${}` shadows a DEFINE PARAM of `{}` with an incompatible `{}`",
                            stmt.name.node,
                            crate::render_kind(&param_kind),
                            crate::render::render_offending(let_kind, Some(&param_kind)),
                        ),
                    )
                    .with_help(
                        "a LET rebinds the param for the rest of this scope; give it a compatible value or rename it",
                    ),
                );
            }
        }
    }
    // Record the binding (name span + inferred kind) for editor features,
    // at whatever nesting depth this LET sits. Drained to the source's
    // top-level env; changes no diagnostics.
    let name_span =
        surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), stmt.name.span);
    ctx.record_let_binding(crate::analysis::LetBindingAnalysis {
        name: stmt.name.node.clone(),
        name_span,
        kind: fact.kind.clone(),
    });
    ctx.define_local(stmt.name.node.clone(), fact);
    // Track `LET $t = type::table($x)` so a later `IF $t = 'table'` guard
    // narrows `$x` (indirect record discriminant).
    ctx.set_table_discriminant(
        stmt.name.node.clone(),
        crate::analyzer::flow::narrow::type_table_arg(&stmt.value.node),
    );
    Kind::None
}

/// Whether a `LET` value's kind is a *provably incompatible* rebinding of a
/// `DEFINE PARAM`'s kind (6002). Only fires when both kinds are known
/// (neither is `any`) and neither is assignable to the other, so a benign
/// narrowing/widening (`int` over `number`) is never flagged.
fn shadows_with_different_kind(let_kind: &Kind, param_kind: &Kind) -> bool {
    if matches!(let_kind, Kind::Any) || matches!(param_kind, Kind::Any) {
        return false;
    }
    !crate::kinds::kind_is_assignable_to(let_kind, param_kind)
        && !crate::kinds::kind_is_assignable_to(param_kind, let_kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_diagnostics::Finding;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    use crate::schema::SchemaIndex;

    #[test]
    fn defines_the_binding_with_the_inferred_kind() {
        let parsed =
            parse_source(SourceId::new("flow:test"), "LET $age = 42;").expect("query parses");
        let ast::Statement::Let(stmt) =
            surrealguard_syntax::lower::lower_first_statement(&parsed, "LetStatement")
                .expect("no LetStatement node in tree")
                .node
        else {
            panic!("expected let statement");
        };
        let schema = SchemaIndex::default();
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(
            &schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
        );

        let kind = analyze_let(&mut ctx, &stmt);

        assert_eq!(kind, Kind::None);
        assert_eq!(
            ctx.env().let_fact("age").and_then(|fact| fact.kind.clone()),
            Some(Kind::Int)
        );
    }
}
