//! `DEFINE TABLE` analysis.
//!
//! A table is defined once: redefining it without `OVERWRITE` is a
//! duplicate definition (1022).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_define_table(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineTable) -> Kind {
    if !stmt.overwrite && ctx.schema().table(&stmt.name.node).is_some() {
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), stmt.name.span);
        ctx.emit(surrealguard_diagnostics::catalog::finding(
            span,
            1022,
            format!("duplicate table definition `{}`", stmt.name.node),
        ));
    }
    Kind::None
}
