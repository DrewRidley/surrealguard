//! `array` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod add;
pub mod all;
pub mod any;
pub mod append;
pub mod at;
pub mod boolean_and;
pub mod boolean_not;
pub mod boolean_or;
pub mod boolean_xor;
pub mod clump;
pub mod combine;
pub mod complement;
pub mod concat;
pub mod difference;
pub mod distinct;
pub mod fill;
pub mod filter;
pub mod filter_index;
pub mod find;
pub mod find_index;
pub mod first;
pub mod flatten;
pub mod fold;
pub mod group;
pub mod insert;
pub mod intersect;
pub mod is_empty;
pub mod join;
pub mod last;
pub mod len;
pub mod logical_and;
pub mod logical_or;
pub mod logical_xor;
pub mod map;
pub mod matches;
pub mod max;
pub mod min;
pub mod pop;
pub mod prepend;
pub mod push;
pub mod range;
pub mod reduce;
pub mod remove;
pub mod repeat;
pub mod reverse;
pub mod sequence;
pub mod shuffle;
pub mod slice;
pub mod sort;
pub mod sort_lexical;
pub mod sort_natural;
pub mod sort_natural_lexical;
pub mod swap;
pub mod transpose;
pub mod union;
pub mod windows;

pub fn analyze_array_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "array::add" => add::analyze_array_add(ctx, call, args),
        "array::all" => all::analyze_array_all(ctx, call, args),
        "array::any" => any::analyze_array_any(ctx, call, args),
        "array::append" => append::analyze_array_append(ctx, call, args),
        "array::at" => at::analyze_array_at(ctx, call, args),
        "array::boolean_and" => boolean_and::analyze_array_boolean_and(ctx, call, args),
        "array::boolean_not" => boolean_not::analyze_array_boolean_not(ctx, call, args),
        "array::boolean_or" => boolean_or::analyze_array_boolean_or(ctx, call, args),
        "array::boolean_xor" => boolean_xor::analyze_array_boolean_xor(ctx, call, args),
        "array::clump" => clump::analyze_array_clump(ctx, call, args),
        "array::combine" => combine::analyze_array_combine(ctx, call, args),
        "array::complement" => complement::analyze_array_complement(ctx, call, args),
        "array::concat" => concat::analyze_array_concat(ctx, call, args),
        "array::difference" => difference::analyze_array_difference(ctx, call, args),
        "array::distinct" => distinct::analyze_array_distinct(ctx, call, args),
        "array::fill" => fill::analyze_array_fill(ctx, call, args),
        "array::filter" => filter::analyze_array_filter(ctx, call, args),
        "array::filter_index" => filter_index::analyze_array_filter_index(ctx, call, args),
        "array::find" => find::analyze_array_find(ctx, call, args),
        "array::find_index" => find_index::analyze_array_find_index(ctx, call, args),
        "array::first" => first::analyze_array_first(ctx, call, args),
        "array::flatten" => flatten::analyze_array_flatten(ctx, call, args),
        "array::fold" => fold::analyze_array_fold(ctx, call, args),
        "array::group" => group::analyze_array_group(ctx, call, args),
        "array::insert" => insert::analyze_array_insert(ctx, call, args),
        "array::intersect" => intersect::analyze_array_intersect(ctx, call, args),
        "array::is_empty" => is_empty::analyze_array_is_empty(ctx, call, args),
        "array::join" => join::analyze_array_join(ctx, call, args),
        "array::last" => last::analyze_array_last(ctx, call, args),
        "array::len" => len::analyze_array_len(ctx, call, args),
        "array::logical_and" => logical_and::analyze_array_logical_and(ctx, call, args),
        "array::logical_or" => logical_or::analyze_array_logical_or(ctx, call, args),
        "array::logical_xor" => logical_xor::analyze_array_logical_xor(ctx, call, args),
        "array::map" => map::analyze_array_map(ctx, call, args),
        "array::matches" => matches::analyze_array_matches(ctx, call, args),
        "array::max" => max::analyze_array_max(ctx, call, args),
        "array::min" => min::analyze_array_min(ctx, call, args),
        "array::pop" => pop::analyze_array_pop(ctx, call, args),
        "array::prepend" => prepend::analyze_array_prepend(ctx, call, args),
        "array::push" => push::analyze_array_push(ctx, call, args),
        "array::range" => range::analyze_array_range(ctx, call, args),
        "array::reduce" => reduce::analyze_array_reduce(ctx, call, args),
        "array::remove" => remove::analyze_array_remove(ctx, call, args),
        "array::repeat" => repeat::analyze_array_repeat(ctx, call, args),
        "array::reverse" => reverse::analyze_array_reverse(ctx, call, args),
        "array::sequence" => sequence::analyze_array_sequence(ctx, call, args),
        "array::shuffle" => shuffle::analyze_array_shuffle(ctx, call, args),
        "array::slice" => slice::analyze_array_slice(ctx, call, args),
        "array::sort" => sort::analyze_array_sort(ctx, call, args),
        "array::sort_lexical" => sort_lexical::analyze_array_sort_lexical(ctx, call, args),
        "array::sort_natural" => sort_natural::analyze_array_sort_natural(ctx, call, args),
        "array::sort_natural_lexical" => {
            sort_natural_lexical::analyze_array_sort_natural_lexical(ctx, call, args)
        }
        "array::swap" => swap::analyze_array_swap(ctx, call, args),
        "array::transpose" => transpose::analyze_array_transpose(ctx, call, args),
        "array::union" => union::analyze_array_union(ctx, call, args),
        "array::windows" => windows::analyze_array_windows(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
