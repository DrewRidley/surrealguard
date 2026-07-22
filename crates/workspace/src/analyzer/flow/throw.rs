//! `THROW` statement analysis: the thrown expression is analyzed for its
//! facts; the statement diverges and produces no value.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_throw(ctx: &mut AnalysisContext<'_>, stmt: &ast::ThrowStmt) -> Kind {
    if let Some(value) = &stmt.value {
        let _ = crate::analyzer::expression::analyze_expr(ctx, value);
    }
    Kind::None
}
