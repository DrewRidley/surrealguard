//! `DEFINE FIELD` analysis.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_define_field(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineField) -> Kind {
    let _ = (ctx, stmt);
    Kind::None
}
