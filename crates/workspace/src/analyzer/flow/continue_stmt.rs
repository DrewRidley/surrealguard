//! `CONTINUE` statement analysis: no value; its one contract is standing
//! inside a `FOR` body (4005).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;
use surrealguard_syntax::span::ByteRange;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_continue(
    ctx: &mut AnalysisContext<'_>,
    stmt: &ast::ContinueStmt,
    span: ByteRange,
) -> Kind {
    let _ = stmt;
    if !ctx.in_loop() {
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), span);
        ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                span,
                4005,
                "CONTINUE here does nothing — it is outside any FOR loop".to_string(),
            )
            .with_help("remove it, or move it inside a `FOR` loop"),
        );
    }
    Kind::None
}
