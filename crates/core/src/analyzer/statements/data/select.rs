use std::collections::HashMap;

use surrealdb::sql::{SelectStatement, Field, Value, Idiom, Tables, Subquery, Query};

use crate::analyzer::{context::AnalyzerContext, model::Type};

pub fn analyze_select(ctx: &AnalyzerContext, stmt: &SelectStatement) -> Type {
    // Get base type from first table in FROM clause
    let base_type = stmt.what.0.first()
        .map_or(Type::unknown(), |v| ctx.resolve(v));

    let mut result_fields = HashMap::new();

    for field in &stmt.expr.0 {
        match field {
            Field::All => return base_type.clone(),
            Field::Single { expr, alias } => {
                let field_type = match expr {
                    Value::Idiom(i) => ctx.resolve_idiom(i),
                    Value::Subquery(s) => match &**s {
                        Subquery::Select(s) => analyze_select(ctx, s),
                        _ => Type::unknown(),
                    },
                    _ => ctx.resolve(expr),
                };

                let field_name = alias.as_ref()
                    .map(|a| a.0.to_string())
                    .unwrap_or_else(|| expr.to_string());

                result_fields.insert(field_name, field_type);
            }
        }
    }

    let result_type = if result_fields.is_empty() {
        base_type
    } else {
        Type::object(result_fields)
    };

    apply_modifiers(stmt, result_type)
}

fn apply_modifiers(stmt: &SelectStatement, mut ty: Type) -> Type {
    // Handle OMIT
    if let Some(omit) = &stmt.omit {
        if let TypeKind::Object(fields) = &mut ty.kind {
            for idiom in &omit.0 {
                if let [Part::Field(f)] = &idiom.0[..] {
                    fields.remove(&f.0);
                }
            }
        }
    }

    // Handle FETCH
    if let Some(fetch) = &stmt.fetch {
        if let TypeKind::Object(fields) = &mut ty.kind {
            for fetch_item in &fetch.0 {
                for part in &fetch_item.0 {
                    if let Part::Field(f) = part {
                        let related = ctx.schema.get_table_type(&f.0)
                            .cloned()
                            .unwrap_or(Type::unknown());
                        fields.insert(f.0.clone(), related);
                    }
                }
            }
        }
    }

    // Handle SPLIT
    if let Some(splits) = &stmt.split {
        if let [split] = splits.0.as_slice() {
            if let [Part::Field(f)] = &split.0[..] {
                if let Some(ty) = ty.as_object().and_then(|fs| fs.get(&f.0)) {
                    return ty.clone();
                }
            }
        }
    }

    // Handle array wrapping
    if !stmt.only {
        Type::array(ty)
    } else {
        ty
    }
}
