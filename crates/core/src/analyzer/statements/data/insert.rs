
use surrealdb::sql::{statements::InsertStatement, Kind};
use crate::analyzer::{context::AnalyzerContext, error::{AnalyzerError, AnalyzerResult}};

/// Analyzes an INSERT statement.
///
/// The logic is similar to UPSERT/UPDATE: resolve the target table and return its full type
/// wrapped in an array.
pub fn analyze_insert(ctx: &mut AnalyzerContext, stmt: &InsertStatement) -> AnalyzerResult<Kind> {
    let what = stmt.into.as_ref().unwrap();

    let kind = ctx.resolve(what)?;
    let target_type = match kind {
        Kind::Record(tables) => {
            if let Some(table) = tables.first() {
                ctx.build_full_table_type(&table.0)?
            } else {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
        },
        _ => return Err(AnalyzerError::UnexpectedSyntax)
    };
    Ok(Kind::Array(Box::new(target_type), None))
}
