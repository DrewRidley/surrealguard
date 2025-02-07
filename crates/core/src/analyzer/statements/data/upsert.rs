use crate::analyzer::{
    context::AnalyzerContext,
    error::{AnalyzerError, AnalyzerResult},
};
use surrealdb::sql::{statements::UpsertStatement, Kind};

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
        }
        _ => return Err(AnalyzerError::UnexpectedSyntax),
    };
    // UPSERT returns an array of the updated records.
    Ok(Kind::Array(Box::new(target_type), None))
}

#[cfg(test)]
mod tests {
    use surrealguard_macros::kind;

    use crate::analyzer::{analyze, context::AnalyzerContext};

    #[test]
    fn upsert_table() {
        let mut ctx = AnalyzerContext::new();
        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
                DEFINE FIELD age ON user TYPE number;
        "#,
        )
        .expect("Schema construction should succeed");

        let stmt = "UPSERT user SET name = 'Jane';";
        let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");

        let expected_kind = kind!("array<array<{ name: string, age: number }>>");
        assert_eq!(analyzed_kind, expected_kind);
    }
}
