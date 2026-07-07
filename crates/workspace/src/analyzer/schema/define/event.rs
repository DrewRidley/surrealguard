//! `DEFINE EVENT` analysis.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_define_event(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineEvent) -> Kind {
    let _ = (ctx, stmt);
    Kind::None
}
