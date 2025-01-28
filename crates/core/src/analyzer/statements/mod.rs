use surrealdb::sql::Statement;

use super::{context::AnalyzerContext, model::{Type, TypeKind}};

mod data;
// mod logic;
mod schema;
// mod system;
// mod transaction;



/// Analyzes an arbitrary string of SurrealQL.
///
/// Returns a Vec containing the type of each statement in order.
pub fn analyze(ctx: &mut AnalyzerContext, surql: &str) -> Vec<Type> {
    let parsed = surrealdb::sql::parse(surql).unwrap();

    parsed.iter().map(|stmt| {
        match stmt {
            Statement::Value(value) => ctx.resolve(value),
            Statement::Select(select_stmt) => self::data::analyze_select(ctx, select_stmt),
            Statement::Define(define_stmt) => self::schema::analyze_define(ctx, define_stmt),
            _ => todo!("Statement not supported.")
        }
    }).collect()
}
