//! `BREAK` statement analysis: no value; its one contract is standing
//! inside a `FOR` body (4005).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;
use surrealguard_syntax::span::ByteRange;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_break(
    ctx: &mut AnalysisContext<'_>,
    stmt: &ast::BreakStmt,
    span: ByteRange,
) -> Kind {
    let _ = stmt;
    if !ctx.in_loop() {
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), span);
        ctx.emit(surrealguard_diagnostics::catalog::finding(
            span,
            4005,
            "BREAK outside a FOR loop does nothing".to_string(),
        ));
    }
    Kind::None
}
