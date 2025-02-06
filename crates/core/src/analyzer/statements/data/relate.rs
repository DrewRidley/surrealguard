use crate::analyzer::{
    context::AnalyzerContext,
    error::{AnalyzerError, AnalyzerResult},
};
use surrealdb::sql::{statements::RelateStatement, Data, Idiom, Kind, Value};

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
    // Extract the relation table name
    let relation_table = match &stmt.kind {
        Value::Table(t) => t.0.clone(),
        Value::Thing(thing) => thing.tb.clone(),
        Value::Idiom(idiom) => idiom.to_string(),
        _ => return Err(AnalyzerError::UnexpectedSyntax),
    };

    // Check 'from' parameter
    if let Value::Param(from_param) = &stmt.from {
        if let Some(from_table) = ctx.get_relation_target(&relation_table, true) {
            ctx.infer_param_from_table(&from_table, from_param)?;
        }
    }

    // Check 'with' parameter
    if let Value::Param(with_param) = &stmt.with {
        if let Some(to_table) = ctx.get_relation_target(&relation_table, false) {
            ctx.infer_param_from_table(&to_table, with_param)?;
        }
    }

    // Handle content parameters if present
    if let Some(data) = &stmt.data {
        match data {
            Data::ContentExpression(value) => {
                match value {
                    Value::Param(param_name) => {
                        // For RELATE ... CONTENT $param
                        ctx.infer_param_from_table(&relation_table, param_name)?;
                    }
                    Value::Object(obj) => {
                        // For RELATE ... CONTENT { field: $param }
                        for (field, value) in obj.iter() {
                            if let Value::Param(param_name) = value {
                                let field_idiom = Idiom::from(field.clone());
                                ctx.infer_param_from_field(&relation_table, &field_idiom, param_name)?;
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    // Build and return the relation type
    let relation_full_type = ctx.build_full_table_type(&relation_table)?;
    Ok(Kind::Array(Box::new(relation_full_type), None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::analyze;
    use surrealdb::sql::Literal;
    use surrealguard_macros::kind;

    #[test]
    fn infer_relate_from_param() {
        let mut ctx = AnalyzerContext::new();
        analyze(&mut ctx, r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
            DEFINE TABLE org SCHEMAFULL;
                DEFINE FIELD name ON org TYPE string;
            DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO org;
                DEFINE FIELD role ON memberOf TYPE string;
        "#).expect("Schema construction should succeed");

        let stmt = "RELATE $person->memberOf->org:acme;";
        analyze(&mut ctx, stmt).expect("Analysis should succeed");

        let params = ctx.get_all_inferred_params();
        assert!(params.iter().any(|(name, kind)| {
            name == "person" && matches!(kind, Kind::Literal(Literal::Object(_)))
        }));
    }

    #[test]
    fn infer_relate_with_param() {
        let mut ctx = AnalyzerContext::new();
        analyze(&mut ctx, r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
            DEFINE TABLE org SCHEMAFULL;
                DEFINE FIELD name ON org TYPE string;
            DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO org;
                DEFINE FIELD role ON memberOf TYPE string;
        "#).expect("Schema construction should succeed");

        let stmt = "RELATE user:john->memberOf->$organization;";
        analyze(&mut ctx, stmt).expect("Analysis should succeed");

        let params = ctx.get_all_inferred_params();
        assert!(params.iter().any(|(name, kind)| {
            name == "organization" && matches!(kind, Kind::Literal(Literal::Object(_)))
        }));
    }

    #[test]
    fn infer_relate_both_params() {
        let mut ctx = AnalyzerContext::new();
        analyze(&mut ctx, r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
            DEFINE TABLE org SCHEMAFULL;
                DEFINE FIELD name ON org TYPE string;
            DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO org;
                DEFINE FIELD role ON memberOf TYPE string;
        "#).expect("Schema construction should succeed");

        let stmt = "RELATE $person->memberOf->$organization;";
        analyze(&mut ctx, stmt).expect("Analysis should succeed");

        let params = ctx.get_all_inferred_params();
        assert_eq!(params.len(), 2);
        assert!(params.iter().any(|(name, _)| name == "person"));
        assert!(params.iter().any(|(name, _)| name == "organization"));
    }

    #[test]
    fn infer_relate_content_fields() {
        let mut ctx = AnalyzerContext::new();
        analyze(&mut ctx, r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
            DEFINE TABLE org SCHEMAFULL;
                DEFINE FIELD name ON org TYPE string;
            DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO org;
                DEFINE FIELD role ON memberOf TYPE string;
        "#).expect("Schema construction should succeed");

        let stmt = r#"
            RELATE user:john->memberOf->org:acme
            CONTENT {
                role: $role
            };
        "#;
        analyze(&mut ctx, stmt).expect("Analysis should succeed");

        let params = ctx.get_all_inferred_params();
        assert!(params.contains(&("role".to_string(), Kind::String)));
    }
}
