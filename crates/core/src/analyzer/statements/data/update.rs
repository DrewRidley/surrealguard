use surrealdb::sql::{
    statements::{DefineStatement, UpdateStatement},
    Data, Idiom, Kind, Value,
};

use crate::analyzer::{
    context::AnalyzerContext,
    error::{AnalyzerError, AnalyzerResult},
};

pub fn analyze_update(ctx: &mut AnalyzerContext, stmt: &UpdateStatement) -> AnalyzerResult<Kind> {
    // Get the table name from the first value
    let kind = ctx.resolve(&stmt.what.0[0])?;
    let table_name = match &kind {
        Kind::Record(tables) => {
            if let Some(table) = tables.first() {
                &table.0
            } else {
                return Err(AnalyzerError::UnexpectedSyntax);
            }
        }
        _ => return Err(AnalyzerError::UnexpectedSyntax),
    };

    // Analyze the Data variant for parameter inference
    if let Some(data) = &stmt.data {
        match data {
            Data::SetExpression(sets) => {
                for (idiom, _op, value) in sets {
                    if let Value::Param(param_name) = value {
                        // For SET expressions, infer from the field being set
                        ctx.infer_param_from_field(table_name, idiom, param_name)?;
                    }
                }
            }
            Data::ContentExpression(value) => {
                match value {
                    Value::Param(param_name) => {
                        // For CONTENT $param, infer the full table type
                        ctx.infer_param_from_table(table_name, param_name)?;
                    }
                    Value::Object(obj) => {
                        // For CONTENT { field: $param }, infer each field's type
                        for (field, value) in obj.iter() {
                            if let Value::Param(param_name) = value {
                                let field_idiom = Idiom::from(field.clone());
                                ctx.infer_param_from_field(table_name, &field_idiom, param_name)?;
                            }
                        }
                    }
                    _ => {}
                }
            }
            Data::MergeExpression(value)
            | Data::PatchExpression(value)
            | Data::ReplaceExpression(value) => {
                match value {
                    Value::Param(param_name) => {
                        // For MERGE/PATCH/REPLACE $param, infer the full table type
                        ctx.infer_param_from_table(table_name, param_name)?;
                    }
                    Value::Object(obj) => {
                        // For MERGE/PATCH/REPLACE { field: $param }, infer each field's type
                        for (field, value) in obj.iter() {
                            if let Value::Param(param_name) = value {
                                let field_idiom = Idiom::from(field.clone());
                                ctx.infer_param_from_field(table_name, &field_idiom, param_name)?;
                            }
                        }
                    }
                    _ => {}
                }
            }
            Data::UpdateExpression(updates) => {
                for (idiom, _op, value) in updates {
                    if let Value::Param(param_name) = value {
                        ctx.infer_param_from_field(table_name, idiom, param_name)?;
                    }
                }
            }
            Data::EmptyExpression
            | Data::UnsetExpression(_)
            | Data::SingleExpression(_)
            | Data::ValuesExpression(_) => {}
            _ => todo!(),
        }
    }

    let target_type = ctx.build_full_table_type(table_name)?;
    Ok(Kind::Array(Box::new(target_type), None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::analyze;
    use surrealdb::sql::Literal;
    use surrealguard_macros::kind;

    #[test]
    fn infer_set_expression() {
        let mut ctx = AnalyzerContext::new();
        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD age ON user TYPE number;
        "#,
        )
        .expect("Schema construction should succeed");

        let stmt = "UPDATE user SET age += $increment;";
        analyze(&mut ctx, stmt).expect("Analysis should succeed");

        let params = ctx.get_all_inferred_params();
        assert!(params.contains(&("increment".to_string(), Kind::Number)));
    }

    #[test]
    fn infer_content_expression() {
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

        let stmt = r#"
            UPDATE user
            CONTENT {
                name: $name,
                age: $age
            };
        "#;
        analyze(&mut ctx, stmt).expect("Analysis should succeed");

        let params = ctx.get_all_inferred_params();
        assert!(params.contains(&("name".to_string(), Kind::String)));
        assert!(params.contains(&("age".to_string(), Kind::Number)));

    }

    #[test]
    fn infer_merge_expression() {
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

        let stmt = "UPDATE user MERGE $data;";
        analyze(&mut ctx, stmt).expect("Analysis should succeed");

        let params = ctx.get_all_inferred_params();
        assert_eq!(params.len(), 1);
        assert!(matches!(params[0].1, Kind::Literal(Literal::Object(_))));
    }

    #[test]
    fn infer_patch_expression() {
        let mut ctx = AnalyzerContext::new();
        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD age ON user TYPE number;
        "#,
        )
        .expect("Schema construction should succeed");

        let stmt = r#"
            UPDATE user
            PATCH {
                age: $new_age
            };
        "#;
        analyze(&mut ctx, stmt).expect("Analysis should succeed");

        let params = ctx.get_all_inferred_params();
        assert!(params.contains(&("new_age".to_string(), Kind::Number)));
    }
}
