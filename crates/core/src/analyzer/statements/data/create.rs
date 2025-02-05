
use surrealdb::sql::{statements::CreateStatement, Kind};
use crate::analyzer::{context::AnalyzerContext, error::{AnalyzerError, AnalyzerResult}};

/// Analyzes a CREATE statement such as:
///
/// ```sql
/// CREATE user CONTENTS { name: 'John', age: 42 };
/// ```
///
/// The analyzer resolves the target table from the `what` clause and builds the full
/// table type from the schema. The returned type is an array wrapping the table type.
pub fn analyze_create(ctx: &mut AnalyzerContext, stmt: &CreateStatement) -> AnalyzerResult<Kind> {
    // Resolve the target table from the first element in the `what` clause.
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

    // CREATE returns an array containing the created record.
    Ok(Kind::Array(Box::new(target_type), None))
}
