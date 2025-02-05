
use surrealdb::sql::{statements::UpsertStatement, Kind};
use crate::analyzer::{context::AnalyzerContext, error::{AnalyzerError, AnalyzerResult}};

/// Analyzes an UPSERT statement.
///
/// The logic is identical to UPDATE: the statement’s `what` clause is resolved to a record,
/// the full table type is built from the target table, and the result is returned as an array.
pub fn analyze_upsert(ctx: &mut AnalyzerContext, stmt: &UpsertStatement) -> AnalyzerResult<Kind> {
    // Resolve the table (using the first value in the `what` clause)
    let kind = ctx.resolve(&stmt.what.0[0])?;
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
    // UPSERT returns an array of the updated records.
    Ok(Kind::Array(Box::new(target_type), None))
}
