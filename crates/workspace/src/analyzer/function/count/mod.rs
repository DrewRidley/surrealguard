//! `count` function family: every built-in it dispatches, with its analyzer.

// One-file-per-function layout: this namespace has a single function
// sharing its name, which is intentional.
#![allow(clippy::module_inception)]

use crate::analyzer::function::BuiltinEntry;

pub mod count;

/// Every `count::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[BuiltinEntry::new(
    "count",
    "Counts the rows in a group, or the truthy values passed to it.",
    count::signature,
    count::analyze_count_count,
)];
