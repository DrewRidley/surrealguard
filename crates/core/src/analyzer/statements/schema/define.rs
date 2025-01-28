use surrealdb::sql::statements::{DefineTableStatement, DefineFieldStatement, DefineIndexStatement, DefineAnalyzerStatement};
use surrealdb::sql::{Expression, Ident, Idioms, Index, Part, Table, Value};
use crate::analyzer::context::AnalyzerContext;
use crate::analyzer::model::{Type, TypeKind};

pub fn analyze_define_table(ctx: &mut AnalyzerContext, stmt: &DefineTableStatement) -> Type {
    let table_type = Type::empty_object();

    // Permissions always exist
    let table_type = table_type.with_permissions(stmt.permissions.clone());

    // Register the table in schema context
    ctx.schema.add_table(stmt.name.to_string(), table_type.clone());

    table_type
}

pub fn analyze_define_field(ctx: &mut AnalyzerContext, stmt: &DefineFieldStatement) -> Type {
    // Get the table type this field belongs to
    let mut table_type = if let Some(table) = ctx.schema.get_table_type(&stmt.what.to_string()).cloned() {
        table
    } else {
        let new_table = Type::empty_object();
        ctx.schema.add_table(stmt.what.to_string(), new_table.clone());
        new_table
    };

    // Resolve field type from kind
    let mut field_type = match &stmt.kind {
        Some(kind) => {
            match kind.to_string().as_str() {
                "string" => Type::string(),
                "int" => Type::int(),
                "float" => Type::float(),
                "bool" => Type::bool(),
                "datetime" => Type::datetime(),
                _ => Type::any()
            }
        }
        None => Type::any()
    };

    // Add field metadata
    field_type = field_type.with_permissions(stmt.permissions.clone());

    if let Some(val) = &stmt.value {
        field_type = field_type.with_default(val.clone());
    }

    // Convert Value to Expression for assert
    if let Some(assert_val) = &stmt.assert {
        // Just store the Value directly since that's what we get from SurrealDB
        field_type = field_type.with_assert(assert_val.clone());
    }

    // Update table type with new field
    if let TypeKind::Object(ref mut fields) = table_type.kind {
        fields.insert(stmt.name.to_string(), field_type.clone());
    }

    ctx.schema.add_table(stmt.what.to_string(), table_type);

    field_type
}

pub fn analyze_define_index(ctx: &mut AnalyzerContext, stmt: &DefineIndexStatement) -> Type {
    let index_def = crate::analyzer::context::indexes::IndexDefinition {
        table: Table::from(stmt.what.to_string()),
        // Access Idiom parts correctly
        fields: stmt.cols.0.iter()
            .filter_map(|idiom| {
                // Idiom.0 is a Vec<Part>
                idiom.0.first().and_then(|part| {
                    if let Part::Field(ident) = part {
                        Some(ident.clone())
                    } else {
                        None
                    }
                })
            })
            .collect(),
        index_type: match stmt.index {
            Index::Idx => crate::analyzer::context::indexes::IndexType::Custom("default".to_string()),
            Index::Uniq => crate::analyzer::context::indexes::IndexType::Unique,
            Index::Search(_) => crate::analyzer::context::indexes::IndexType::Search,
            // Add any other index types SurrealDB supports
            _ => crate::analyzer::context::indexes::IndexType::Custom("unknown".to_string()),
        }
    };

    ctx.indexes.add_index(stmt.name.to_string(), index_def);
    Type::empty_object()
}
