//! `BREAK` statement analysis. No value; loop-context invariants (BREAK
//! outside a loop) belong here.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_break(ctx: &mut AnalysisContext<'_>, stmt: &ast::BreakStmt) -> Kind {
    let _ = (ctx, stmt);
    Kind::None
}
