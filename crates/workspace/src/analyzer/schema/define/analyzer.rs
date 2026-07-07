//! `DEFINE ANALYZER` analysis.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_define_analyzer(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineAnalyzer) -> Kind {
    let _ = (ctx, stmt);
    Kind::None
}
