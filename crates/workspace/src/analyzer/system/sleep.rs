//! `SLEEP` statement analysis. Produces no value.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_sleep(ctx: &mut AnalysisContext<'_>, stmt: &ast::SleepStmt) -> Kind {
    let _ = (ctx, stmt);
    Kind::None
}
