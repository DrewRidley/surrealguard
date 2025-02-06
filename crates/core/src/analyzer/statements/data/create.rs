use crate::analyzer::{
    context::AnalyzerContext,
    error::{AnalyzerError, AnalyzerResult},
};
use surrealdb::sql::{statements::CreateStatement, Data, Idiom, Kind, Value};

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
    let table_name = match kind {
        Kind::Record(tables) => {
            if let Some(table) = tables.first() {
                table.0.clone() // Clone the String instead of referencing it
            } else {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
        },
        _ => return Err(AnalyzerError::UnexpectedSyntax)
    };

    // Analyze the Data variant for parameter inference
    if let Some(data) = &stmt.data {
        match data {
            Data::ContentExpression(value) => {
                match value {
                    Value::Param(param_name) => {
                        // For CREATE ... CONTENT $record
                        ctx.infer_param_from_table(&table_name, param_name)?;
                    }
                    Value::Object(obj) => {
                        // For CREATE ... CONTENT { field: $param }
                        for (field, value) in obj.iter() {
                            if let Value::Param(param_name) = value {
                                let field_idiom = Idiom::from(field.clone());
                                ctx.infer_param_from_field(&table_name, &field_idiom, param_name)?;
                            }
                        }
                    }
                    _ => {}
                }
            }
            Data::SingleExpression(value) => {
                // Handle single value expressions
                if let Value::Param(param_name) = value {
                    ctx.infer_param_from_table(&table_name, param_name)?;
                }
            }
            Data::ValuesExpression(values) => {
                // Handle VALUES (...) syntax
                for value_set in values {
                    for (idiom, value) in value_set {
                        if let Value::Param(param_name) = value {
                            ctx.infer_param_from_field(&table_name, idiom, param_name)?;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let target_type = ctx.build_full_table_type(&table_name)?;
    Ok(Kind::Array(Box::new(target_type), None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::analyze;
    use surrealdb::sql::Literal;
    use surrealguard_macros::kind;

    #[test]
    fn infer_create_content_param() {
        let mut ctx = AnalyzerContext::new();
        analyze(&mut ctx, r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD email ON user TYPE string;
                DEFINE FIELD age ON user TYPE number;
        "#).expect("Schema construction should succeed");

        let stmt = "CREATE user CONTENT $user;";
        analyze(&mut ctx, stmt).expect("Analysis should succeed");

        let params = ctx.get_all_inferred_params();
        assert_eq!(params.len(), 1);
        assert!(matches!(params[0].1, Kind::Literal(Literal::Object(_))));
    }

    #[test]
    fn infer_create_content_fields() {
        let mut ctx = AnalyzerContext::new();
        analyze(&mut ctx, r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD email ON user TYPE string;
                DEFINE FIELD age ON user TYPE number;
        "#).expect("Schema construction should succeed");

        let stmt = r#"
            CREATE user CONTENT {
                email: $email,
                age: $age
            };
        "#;
        analyze(&mut ctx, stmt).expect("Analysis should succeed");

        let params = ctx.get_all_inferred_params();
        assert!(params.contains(&("email".to_string(), Kind::String)));
        assert!(params.contains(&("age".to_string(), Kind::Number)));
    }

}
