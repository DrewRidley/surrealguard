//! `value` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

pub mod diff;
pub mod expect;
pub mod patch;

/// Every `value::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "value::diff",
        "The JSON Patch operations that turn one value into another.",
        diff::signature,
        diff::analyze_value_diff,
    ),
    BuiltinEntry::new(
        "value::expect",
        "Returns the value, or throws the given error when it is NONE.",
        expect::signature,
        expect::analyze_value_expect,
    ),
    BuiltinEntry::new(
        "value::patch",
        "Applies JSON Patch operations to a value.",
        patch::signature,
        patch::analyze_value_patch,
    ),
];
