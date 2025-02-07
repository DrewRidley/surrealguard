use crate::analyzer::{
    context::AnalyzerContext,
    error::{AnalyzerError, AnalyzerResult},
};
use surrealdb::sql::{statements::InsertStatement, Kind};

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
        }
        _ => return Err(AnalyzerError::UnexpectedSyntax),
    };
    Ok(Kind::Array(Box::new(target_type), None))
}

#[cfg(test)]
mod tests {
    use surrealguard_macros::kind;

    use crate::analyzer::{analyze, context::AnalyzerContext};

    #[test]
    fn insert_table() {
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

        let stmt = "INSERT INTO user { name: 'Jane', age: 30 };";
        let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");

        let expected_kind = kind!("array<array<{ name: string, age: number }>>");
        assert_eq!(analyzed_kind, expected_kind);
    }
}
