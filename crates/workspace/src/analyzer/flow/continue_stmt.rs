//! `CONTINUE` statement analysis. No value; loop-context invariants
//! (CONTINUE outside a loop) belong here.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_continue(ctx: &mut AnalysisContext<'_>, stmt: &ast::ContinueStmt) -> Kind {
    let _ = (ctx, stmt);
    Kind::None
}
