//! `DEFINE PARAM` analysis.
//!
//! Contracts: the definition is made once (1022), and it gives `$name` a
//! database-side default, so later reads are neither unknown nor
//! host-required — they carry the default's kind unless the host overrides.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_define_param(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineParam) -> Kind {
    if !stmt.overwrite && !stmt.if_not_exists {
        let existing = ctx
            .schema()
            .param(&stmt.name.node)
            .map(|existing| existing.name_span.clone());
        if let Some(existing) = existing {
            super::emit_duplicate_definition(
                ctx,
                stmt.name.span,
                &format!("`${}`", stmt.name.node),
                &format!("DEFINE PARAM OVERWRITE ${}", stmt.name.node),
                existing,
            );
        }
    }
    if let Some(value) = &stmt.value {
        let fact = crate::analyzer::expression::expr_fact(ctx, value);
        ctx.define_param_default(stmt.name.node.clone(), fact);
    }
    Kind::None
}
