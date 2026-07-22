//! `OPTION` statement analysis. Produces no value.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_option(ctx: &mut AnalysisContext<'_>, stmt: &ast::OptionStmt) -> Kind {
    let _ = (ctx, stmt);
    Kind::None
}
