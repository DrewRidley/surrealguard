use crate::analyzer::{context::AnalyzerContext, error::AnalyzerResult};
use surrealdb::sql::{statements::DeleteStatement, Kind, Literal};

/// Analyzes a DELETE statement.
///
/// As per our design DELETE always returns an empty array.
pub fn analyze_delete(_ctx: &mut AnalyzerContext, _stmt: &DeleteStatement) -> AnalyzerResult<Kind> {
    Ok(Kind::Literal(Literal::Array(vec![])))
}

#[cfg(test)]
mod tests {
    use surrealguard_macros::kind;

    use crate::analyzer::{analyze, context::AnalyzerContext};

    #[test]
    fn delete_table() {
        let mut ctx = AnalyzerContext::new();
        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
        "#,
        )
        .expect("Schema construction should succeed");

        let stmt = "DELETE user WHERE name = 'Jane';";
        let analyzed_kind = analyze(&mut ctx, stmt).expect("Analysis should succeed");

        let expected_kind = kind!("array<[]>");
        assert_eq!(analyzed_kind, expected_kind);
    }
}
