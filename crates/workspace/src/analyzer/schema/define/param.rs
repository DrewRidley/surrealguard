//! `DEFINE PARAM` analysis.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_define_param(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineParam) -> Kind {
    let _ = (ctx, stmt);
    Kind::None
}
