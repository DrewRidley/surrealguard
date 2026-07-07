//! `set` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod add;
pub mod all;
pub mod any;
pub mod at;
pub mod complement;
pub mod contains;
pub mod difference;
pub mod filter;
pub mod find;
pub mod first;
pub mod flatten;
pub mod fold;
pub mod intersect;
pub mod is_empty;
pub mod join;
pub mod last;
pub mod len;
pub mod map;
pub mod max;
pub mod min;
pub mod reduce;
pub mod remove;
pub mod slice;
pub mod union;

pub fn analyze_set_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "set::add" => add::analyze_set_add(ctx, call, args),
        "set::all" => all::analyze_set_all(ctx, call, args),
        "set::any" => any::analyze_set_any(ctx, call, args),
        "set::at" => at::analyze_set_at(ctx, call, args),
        "set::complement" => complement::analyze_set_complement(ctx, call, args),
        "set::contains" => contains::analyze_set_contains(ctx, call, args),
        "set::difference" => difference::analyze_set_difference(ctx, call, args),
        "set::filter" => filter::analyze_set_filter(ctx, call, args),
        "set::find" => find::analyze_set_find(ctx, call, args),
        "set::first" => first::analyze_set_first(ctx, call, args),
        "set::flatten" => flatten::analyze_set_flatten(ctx, call, args),
        "set::fold" => fold::analyze_set_fold(ctx, call, args),
        "set::intersect" => intersect::analyze_set_intersect(ctx, call, args),
        "set::is_empty" => is_empty::analyze_set_is_empty(ctx, call, args),
        "set::join" => join::analyze_set_join(ctx, call, args),
        "set::last" => last::analyze_set_last(ctx, call, args),
        "set::len" => len::analyze_set_len(ctx, call, args),
        "set::map" => map::analyze_set_map(ctx, call, args),
        "set::max" => max::analyze_set_max(ctx, call, args),
        "set::min" => min::analyze_set_min(ctx, call, args),
        "set::reduce" => reduce::analyze_set_reduce(ctx, call, args),
        "set::remove" => remove::analyze_set_remove(ctx, call, args),
        "set::slice" => slice::analyze_set_slice(ctx, call, args),
        "set::union" => union::analyze_set_union(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
