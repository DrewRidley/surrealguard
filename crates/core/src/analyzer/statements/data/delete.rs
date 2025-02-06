use crate::analyzer::{
    context::AnalyzerContext,
    error::{AnalyzerError, AnalyzerResult},
};
use surrealdb::sql::{statements::DeleteStatement, Kind, Literal};

/// Analyzes a DELETE statement.
///
/// As per our design DELETE always returns an empty array.
pub fn analyze_delete(_ctx: &mut AnalyzerContext, _stmt: &DeleteStatement) -> AnalyzerResult<Kind> {
    Ok(Kind::Literal(Literal::Array(vec![])))
}
