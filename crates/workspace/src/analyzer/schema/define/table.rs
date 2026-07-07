//! `DEFINE TABLE` analysis.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_define_table(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineTable) -> Kind {
    let _ = (ctx, stmt);
    Kind::None
}
