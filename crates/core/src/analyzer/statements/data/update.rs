use surrealdb::sql::{statements::{DefineStatement, UpdateStatement}, Kind};

use crate::analyzer::{context::AnalyzerContext, error::{AnalyzerError, AnalyzerResult}};

pub fn analyze_update(ctx: &mut AnalyzerContext, stmt: &UpdateStatement) -> AnalyzerResult<Kind> {
    // Get the table name from the first value
    let kind = ctx.resolve(&stmt.what.0[0])?;

    let target_type = match kind {
        Kind::Record(tables) => {
            if let Some(table) = tables.first() {
                ctx.build_full_table_type(&table.0)?
            } else {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
        }
        _ => return Err(AnalyzerError::UnexpectedSyntax)
    };

    // Wrap in array since UPDATE returns [{ fields... }]
    Ok(Kind::Array(Box::new(target_type), None))
}
