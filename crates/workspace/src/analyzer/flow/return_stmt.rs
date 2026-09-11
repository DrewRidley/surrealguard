//! `RETURN` statement analysis: the statement's value is the returned
//! expression's inferred kind.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_return(ctx: &mut AnalysisContext<'_>, stmt: &ast::ReturnStmt) -> Kind {
    match &stmt.value {
        Some(value) => crate::analyzer::expression::analyze_expr(ctx, value),
        None => Kind::None,
    }
}
