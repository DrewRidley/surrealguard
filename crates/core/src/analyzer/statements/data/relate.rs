
use surrealdb::sql::{statements::RelateStatement, Kind, Value};
use crate::analyzer::{context::AnalyzerContext, error::{AnalyzerError, AnalyzerResult}};

/// Analyzes a RELATE statement.
///
/// For a statement such as:
///
/// ```sql
/// RELATE user -> memberOf -> org;
/// ```
///
/// the relation table is specified in the `kind` field of the statement.
/// This function extracts that table name, builds its full type from the schema,
/// and returns the type wrapped in an array.
pub fn analyze_relate(ctx: &mut AnalyzerContext, stmt: &RelateStatement) -> AnalyzerResult<Kind> {
    // Extract the relation table name from the `kind` field.
    let relation_table = match &stmt.kind {
        Value::Table(t) => t.0.clone(),
        Value::Thing(thing) => thing.tb.clone(),
        Value::Idiom(idiom) => idiom.to_string(),
        _ => return Err(AnalyzerError::UnexpectedSyntax),
    };

    // Build the full table type using the analyzer context.
    let relation_full_type = ctx.build_full_table_type(&relation_table)?;

    // Return the relation type wrapped in an array.
    Ok(Kind::Array(Box::new(relation_full_type), None))
}
