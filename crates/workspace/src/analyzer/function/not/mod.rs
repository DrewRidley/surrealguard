//! `not` function family: every built-in it dispatches, with its analyzer.

// One-file-per-function layout: this namespace has a single function
// sharing its name, which is intentional.
#![allow(clippy::module_inception)]

use crate::analyzer::function::BuiltinEntry;

pub mod not;

/// Every `not` built-in the analyzer resolves, in dispatch order. The only
/// spelling is the bare `not(x)` (`not::not(x)` is "Invalid function/constant
/// path" on 3.2.3); `NOT (x)` reaches it because function names fold to
/// lowercase in lowering.
pub(crate) static CATALOG: &[BuiltinEntry] = &[BuiltinEntry::new(
    "not",
    "The logical negation of a value's truthiness.",
    not::signature,
    not::analyze_not_not,
)];
