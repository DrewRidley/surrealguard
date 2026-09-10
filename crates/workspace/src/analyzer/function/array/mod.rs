//! `array` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

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
pub mod sort_asc;
pub mod sort_desc;
pub mod sort_lexical;
pub mod sort_natural;
pub mod sort_natural_lexical;
pub mod swap;
pub mod transpose;
pub mod union;
pub mod windows;

/// Every `array::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "array::add",
        "Adds a value to the array if it is not already present.",
        add::signature,
        add::analyze_array_add,
    ),
    BuiltinEntry::new(
        "array::all",
        "Whether every element is truthy.",
        all::signature,
        all::analyze_array_all,
    ),
    BuiltinEntry::new(
        "array::every",
        "Whether every element is truthy (alias of `array::all`).",
        all::signature,
        all::analyze_array_all,
    ),
    BuiltinEntry::new(
        "array::any",
        "Whether any element is truthy.",
        any::signature,
        any::analyze_array_any,
    ),
    BuiltinEntry::new(
        "array::some",
        "Whether any element is truthy (alias of `array::any`).",
        any::signature,
        any::analyze_array_any,
    ),
    BuiltinEntry::new(
        "array::includes",
        "Whether any element is truthy (alias of `array::any`).",
        any::signature,
        any::analyze_array_any,
    ),
    BuiltinEntry::new(
        "array::append",
        "Appends a value to the end of the array.",
        append::signature,
        append::analyze_array_append,
    ),
    BuiltinEntry::new(
        "array::at",
        "The element at the given index; negative indices count from the end.",
        at::signature,
        at::analyze_array_at,
    ),
    BuiltinEntry::new(
        "array::boolean_and",
        "Element-wise logical AND of two arrays, as booleans.",
        boolean_and::signature,
        boolean_and::analyze_array_boolean_and,
    ),
    BuiltinEntry::new(
        "array::boolean_not",
        "Element-wise logical NOT of an array, as booleans.",
        boolean_not::signature,
        boolean_not::analyze_array_boolean_not,
    ),
    BuiltinEntry::new(
        "array::boolean_or",
        "Element-wise logical OR of two arrays, as booleans.",
        boolean_or::signature,
        boolean_or::analyze_array_boolean_or,
    ),
    BuiltinEntry::new(
        "array::boolean_xor",
        "Element-wise logical XOR of two arrays, as booleans.",
        boolean_xor::signature,
        boolean_xor::analyze_array_boolean_xor,
    ),
    BuiltinEntry::new(
        "array::clump",
        "Splits the array into consecutive chunks of the given size.",
        clump::signature,
        clump::analyze_array_clump,
    ),
    BuiltinEntry::new(
        "array::combine",
        "Every pairing of the two arrays' elements, as two-element arrays.",
        combine::signature,
        combine::analyze_array_combine,
    ),
    BuiltinEntry::new(
        "array::complement",
        "The elements of the first array that are not in the second.",
        complement::signature,
        complement::analyze_array_complement,
    ),
    BuiltinEntry::new(
        "array::concat",
        "Concatenates the arrays, keeping duplicates.",
        concat::signature,
        concat::analyze_array_concat,
    ),
    BuiltinEntry::new(
        "array::difference",
        "The elements present in exactly one of the two arrays.",
        difference::signature,
        difference::analyze_array_difference,
    ),
    BuiltinEntry::new(
        "array::distinct",
        "The array with duplicate values removed.",
        distinct::signature,
        distinct::analyze_array_distinct,
    ),
    BuiltinEntry::new(
        "array::fill",
        "Overwrites the elements in the given range with a value.",
        fill::signature,
        fill::analyze_array_fill,
    ),
    BuiltinEntry::new(
        "array::filter",
        "The elements for which the closure returns true.",
        filter::signature,
        filter::analyze_array_filter,
    ),
    BuiltinEntry::new(
        "array::filter_index",
        "The indexes of the elements that match the value or predicate.",
        filter_index::signature,
        filter_index::analyze_array_filter_index,
    ),
    BuiltinEntry::new(
        "array::find",
        "The first element that matches the value or predicate, or NONE.",
        find::signature,
        find::analyze_array_find,
    ),
    BuiltinEntry::new(
        "array::find_index",
        "The index of the first element that matches the value or predicate, or NONE.",
        find_index::signature,
        find_index::analyze_array_find_index,
    ),
    BuiltinEntry::new(
        "array::index_of",
        "The index of the first element that matches the value or predicate (alias of `array::find_index`).",
        find_index::signature,
        find_index::analyze_array_find_index,
    ),
    BuiltinEntry::new(
        "array::first",
        "The first element of the array, or NONE.",
        first::signature,
        first::analyze_array_first,
    ),
    BuiltinEntry::new(
        "array::flatten",
        "Flattens one level of nested arrays.",
        flatten::signature,
        flatten::analyze_array_flatten,
    ),
    BuiltinEntry::new(
        "array::fold",
        "Reduces the array with a closure over an accumulator, starting from an initial value.",
        fold::signature,
        fold::analyze_array_fold,
    ),
    BuiltinEntry::new(
        "array::group",
        "Flattens nested arrays and returns the unique values.",
        group::signature,
        group::analyze_array_group,
    ),
    BuiltinEntry::new(
        "array::insert",
        "Inserts a value at the given index, shifting the rest.",
        insert::signature,
        insert::analyze_array_insert,
    ),
    BuiltinEntry::new(
        "array::intersect",
        "The elements present in both arrays.",
        intersect::signature,
        intersect::analyze_array_intersect,
    ),
    BuiltinEntry::new(
        "array::is_empty",
        "Whether the array has no elements.",
        is_empty::signature,
        is_empty::analyze_array_is_empty,
    ),
    BuiltinEntry::new(
        "array::join",
        "Joins the elements into a string with the given separator.",
        join::signature,
        join::analyze_array_join,
    ),
    BuiltinEntry::new(
        "array::last",
        "The last element of the array, or NONE.",
        last::signature,
        last::analyze_array_last,
    ),
    BuiltinEntry::new(
        "array::len",
        "The number of elements in the array.",
        len::signature,
        len::analyze_array_len,
    ),
    BuiltinEntry::new(
        "array::logical_and",
        "Element-wise logical AND of two arrays, keeping the operand values.",
        logical_and::signature,
        logical_and::analyze_array_logical_and,
    ),
    BuiltinEntry::new(
        "array::logical_or",
        "Element-wise logical OR of two arrays, keeping the operand values.",
        logical_or::signature,
        logical_or::analyze_array_logical_or,
    ),
    BuiltinEntry::new(
        "array::logical_xor",
        "Element-wise logical XOR of two arrays, keeping the operand values.",
        logical_xor::signature,
        logical_xor::analyze_array_logical_xor,
    ),
    BuiltinEntry::new(
        "array::map",
        "Applies the closure to every element and returns the results.",
        map::signature,
        map::analyze_array_map,
    ),
    BuiltinEntry::new(
        "array::matches",
        "Whether each element equals the given value, as booleans.",
        matches::signature,
        matches::analyze_array_matches,
    ),
    BuiltinEntry::new(
        "array::max",
        "The greatest element of the array.",
        max::signature,
        max::analyze_array_max,
    ),
    BuiltinEntry::new(
        "array::min",
        "The smallest element of the array.",
        min::signature,
        min::analyze_array_min,
    ),
    BuiltinEntry::new(
        "array::pop",
        "Removes and returns the last element.",
        pop::signature,
        pop::analyze_array_pop,
    ),
    BuiltinEntry::new(
        "array::prepend",
        "Inserts a value at the start of the array.",
        prepend::signature,
        prepend::analyze_array_prepend,
    ),
    BuiltinEntry::new(
        "array::push",
        "Appends a value to the end of the array.",
        push::signature,
        push::analyze_array_push,
    ),
    BuiltinEntry::new(
        "array::range",
        "An array of consecutive integers from a start, of the given length.",
        range::signature,
        range::analyze_array_range,
    ),
    BuiltinEntry::new(
        "array::reduce",
        "Reduces the array with a closure over an accumulator seeded by the first element.",
        reduce::signature,
        reduce::analyze_array_reduce,
    ),
    BuiltinEntry::new(
        "array::remove",
        "Removes the element at the given index.",
        remove::signature,
        remove::analyze_array_remove,
    ),
    BuiltinEntry::new(
        "array::repeat",
        "An array of the value repeated the given number of times.",
        repeat::signature,
        repeat::analyze_array_repeat,
    ),
    BuiltinEntry::new(
        "array::reverse",
        "The array in reverse order.",
        reverse::signature,
        reverse::analyze_array_reverse,
    ),
    BuiltinEntry::new(
        "array::sequence",
        "An array of consecutive integers from a start, of the given length.",
        sequence::signature,
        sequence::analyze_array_sequence,
    ),
    BuiltinEntry::new(
        "array::shuffle",
        "The array in random order.",
        shuffle::signature,
        shuffle::analyze_array_shuffle,
    ),
    BuiltinEntry::new(
        "array::slice",
        "A sub-array from a start index, of the given length.",
        slice::signature,
        slice::analyze_array_slice,
    ),
    BuiltinEntry::new(
        "array::sort",
        "The array sorted ascending, or by the given direction.",
        sort::signature,
        sort::analyze_array_sort,
    ),
    BuiltinEntry::new(
        "array::sort_lexical",
        "The array sorted as strings.",
        sort_lexical::signature,
        sort_lexical::analyze_array_sort_lexical,
    ),
    BuiltinEntry::new(
        "array::sort_natural",
        "The array sorted with numbers compared numerically.",
        sort_natural::signature,
        sort_natural::analyze_array_sort_natural,
    ),
    BuiltinEntry::new(
        "array::sort_natural_lexical",
        "The array sorted as strings, with numbers compared numerically.",
        sort_natural_lexical::signature,
        sort_natural_lexical::analyze_array_sort_natural_lexical,
    ),
    BuiltinEntry::new(
        "array::swap",
        "Swaps the elements at two indexes.",
        swap::signature,
        swap::analyze_array_swap,
    ),
    BuiltinEntry::new(
        "array::transpose",
        "Transposes an array of arrays, zipping the columns.",
        transpose::signature,
        transpose::analyze_array_transpose,
    ),
    BuiltinEntry::new(
        "array::union",
        "The unique elements of both arrays.",
        union::signature,
        union::analyze_array_union,
    ),
    BuiltinEntry::new(
        "array::windows",
        "Every overlapping window of the given size.",
        windows::signature,
        windows::analyze_array_windows,
    ),
    BuiltinEntry::new(
        "array::sort::asc",
        "The array sorted in ascending order.",
        sort_asc::signature,
        sort_asc::analyze_array_sort_asc,
    ),
    BuiltinEntry::new(
        "array::sort::desc",
        "The array sorted in descending order.",
        sort_desc::signature,
        sort_desc::analyze_array_sort_desc,
    ),
];
