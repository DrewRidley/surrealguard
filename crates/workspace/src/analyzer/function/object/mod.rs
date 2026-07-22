//! `object` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod entries;
pub mod extend;
pub mod from_entries;
pub mod is_empty;
pub mod keys;
pub mod len;
pub mod remove;
pub mod values;

pub(crate) fn analyze_object_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "object::entries" => entries::analyze_object_entries(ctx, call, args),
        "object::extend" => extend::analyze_object_extend(ctx, call, args),
        "object::from_entries" => from_entries::analyze_object_from_entries(ctx, call, args),
        "object::is_empty" => is_empty::analyze_object_is_empty(ctx, call, args),
        "object::keys" => keys::analyze_object_keys(ctx, call, args),
        "object::len" => len::analyze_object_len(ctx, call, args),
        "object::remove" => remove::analyze_object_remove(ctx, call, args),
        "object::values" => values::analyze_object_values(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
