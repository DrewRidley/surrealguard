//! `DEFINE INDEX` analysis.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_define_index(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineIndex) -> Kind {
    let _ = (ctx, stmt);
    Kind::None
}
