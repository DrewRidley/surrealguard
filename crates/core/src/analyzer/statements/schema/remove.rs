use surrealdb::sql::statements::{RemoveTableStatement, RemoveFieldStatement, RemoveIndexStatement, RemoveAnalyzerStatement};
use crate::analyzer::context::AnalyzerContext;
use crate::analyzer::model::{Type, TypeKind};

pub fn analyze_remove_table(ctx: &mut AnalyzerContext, stmt: &RemoveTableStatement) -> Type {
    ctx.schema.tables.remove(&stmt.name.to_string());
    Type::empty_object()
}

pub fn analyze_remove_field(ctx: &mut AnalyzerContext, stmt: &RemoveFieldStatement) -> Type {
    if let Some(table_type) = ctx.schema.get_table_type(&stmt.what.to_string()).cloned() {
        if let TypeKind::Object(mut fields) = table_type.kind {
            fields.remove(&stmt.name.to_string());
            let new_table_type = Type::object(fields);
            ctx.schema.add_table(stmt.what.to_string(), new_table_type);
        }
    }
    Type::empty_object()
}

pub fn analyze_remove_index(ctx: &mut AnalyzerContext, stmt: &RemoveIndexStatement) -> Type {
    // Use a public method to remove index instead of accessing private field
    ctx.indexes.remove_index(&stmt.name.to_string());
    Type::empty_object()
}

pub fn analyze_remove_analyzer(ctx: &mut AnalyzerContext, stmt: &RemoveAnalyzerStatement) -> Type {
    ctx.schema.analyzers.remove(&stmt.name.to_string());
    Type::empty_object()
}
