//! `set` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

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

/// Every `set::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "set::add",
        "Adds a value to the set.",
        add::signature,
        add::analyze_set_add,
    ),
    BuiltinEntry::new(
        "set::all",
        "Whether every element is truthy.",
        all::signature,
        all::analyze_set_all,
    ),
    BuiltinEntry::new(
        "set::any",
        "Whether any element is truthy.",
        any::signature,
        any::analyze_set_any,
    ),
    BuiltinEntry::new(
        "set::at",
        "The element at the given index.",
        at::signature,
        at::analyze_set_at,
    ),
    BuiltinEntry::new(
        "set::complement",
        "The elements of the first set that are not in the second.",
        complement::signature,
        complement::analyze_set_complement,
    ),
    BuiltinEntry::new(
        "set::contains",
        "Whether the set contains the value.",
        contains::signature,
        contains::analyze_set_contains,
    ),
    BuiltinEntry::new(
        "set::difference",
        "The elements present in exactly one of the two sets.",
        difference::signature,
        difference::analyze_set_difference,
    ),
    BuiltinEntry::new(
        "set::filter",
        "The elements for which the closure returns true.",
        filter::signature,
        filter::analyze_set_filter,
    ),
    BuiltinEntry::new(
        "set::find",
        "The first element that matches the value or predicate, or NONE.",
        find::signature,
        find::analyze_set_find,
    ),
    BuiltinEntry::new(
        "set::first",
        "The first element of the set, or NONE.",
        first::signature,
        first::analyze_set_first,
    ),
    BuiltinEntry::new(
        "set::flatten",
        "Flattens one level of nested collections.",
        flatten::signature,
        flatten::analyze_set_flatten,
    ),
    BuiltinEntry::new(
        "set::fold",
        "Reduces the set with a closure over an accumulator, starting from an initial value.",
        fold::signature,
        fold::analyze_set_fold,
    ),
    BuiltinEntry::new(
        "set::intersect",
        "The elements present in both sets.",
        intersect::signature,
        intersect::analyze_set_intersect,
    ),
    BuiltinEntry::new(
        "set::is_empty",
        "Whether the set has no elements.",
        is_empty::signature,
        is_empty::analyze_set_is_empty,
    ),
    BuiltinEntry::new(
        "set::join",
        "Joins the elements into a string with the given separator.",
        join::signature,
        join::analyze_set_join,
    ),
    BuiltinEntry::new(
        "set::last",
        "The last element of the set, or NONE.",
        last::signature,
        last::analyze_set_last,
    ),
    BuiltinEntry::new(
        "set::len",
        "The number of elements in the set.",
        len::signature,
        len::analyze_set_len,
    ),
    BuiltinEntry::new(
        "set::map",
        "Applies the closure to every element and returns the results.",
        map::signature,
        map::analyze_set_map,
    ),
    BuiltinEntry::new(
        "set::max",
        "The greatest element of the set.",
        max::signature,
        max::analyze_set_max,
    ),
    BuiltinEntry::new(
        "set::min",
        "The smallest element of the set.",
        min::signature,
        min::analyze_set_min,
    ),
    BuiltinEntry::new(
        "set::reduce",
        "Reduces the set with a closure over an accumulator seeded by the first element.",
        reduce::signature,
        reduce::analyze_set_reduce,
    ),
    BuiltinEntry::new(
        "set::remove",
        "The set without the given value.",
        remove::signature,
        remove::analyze_set_remove,
    ),
    BuiltinEntry::new(
        "set::slice",
        "A sub-set from a start index, of the given length.",
        slice::signature,
        slice::analyze_set_slice,
    ),
    BuiltinEntry::new(
        "set::union",
        "The unique elements of both sets.",
        union::signature,
        union::analyze_set_union,
    ),
];
