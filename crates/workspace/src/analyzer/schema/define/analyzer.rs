//! `DEFINE ANALYZER` analysis.
//!
//! A full-text pipeline names known tokenizers and filters, with valid
//! filter arguments (1032/2035).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::schema::SchemaIndex;

pub(crate) fn analyze_define_analyzer(
    ctx: &mut AnalysisContext<'_>,
    stmt: &ast::DefineAnalyzer,
) -> Kind {
    let analyzer = crate::schema::analyzer_def_from_ast(stmt, ctx.source());
    for finding in SchemaIndex::validate_analyzer(&analyzer) {
        ctx.emit(finding);
    }
    Kind::None
}
