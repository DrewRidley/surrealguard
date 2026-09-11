//! `BREAK` statement analysis: no value; its one contract is standing
//! inside a `FOR` body (4005).

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;
use surrealql_analyzer_syntax::span::ByteRange;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_break(
    ctx: &mut AnalysisContext<'_>,
    stmt: &ast::BreakStmt,
    span: ByteRange,
) -> Kind {
    let _ = stmt;
    if !ctx.in_loop() {
        let span = surrealql_analyzer_syntax::span::SourceSpan::new(ctx.source().clone(), span);
        ctx.emit(
            surrealql_analyzer_diagnostics::catalog::finding(
                span,
                4005,
                "BREAK here does nothing — it is outside any FOR loop".to_string(),
            )
            .with_help("remove it, or move it inside a `FOR` loop"),
        );
    }
    Kind::None
}
