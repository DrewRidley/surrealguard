mod define;
mod remove;

use surrealdb::sql::statements::{
    DefineStatement,
    DefineTableStatement,
    DefineFieldStatement,
    DefineIndexStatement,
    DefineAnalyzerStatement,
    // Add other define statements as needed
};
use crate::analyzer::context::AnalyzerContext;
use crate::analyzer::model::Type;

pub use define::*;
pub use remove::*;

/// Routes DEFINE statements to their specific analyzers
pub fn analyze_define(ctx: &mut AnalyzerContext, stmt: &DefineStatement) -> Type {
    match stmt {
        DefineStatement::Table(table_stmt) => analyze_define_table(ctx, table_stmt),
        DefineStatement::Field(field_stmt) => analyze_define_field(ctx, field_stmt),
        DefineStatement::Index(index_stmt) => analyze_define_index(ctx, index_stmt),
        _ => todo!("Define Statement not supported.")
    }
}
