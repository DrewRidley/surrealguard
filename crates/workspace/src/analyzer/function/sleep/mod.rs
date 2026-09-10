//! `sleep` function family: every built-in it dispatches, with its analyzer.

// One-file-per-function layout: this namespace has a single function
// sharing its name, which is intentional.
#![allow(clippy::module_inception)]

use crate::analyzer::function::BuiltinEntry;

pub mod sleep;

/// Every `sleep::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[BuiltinEntry::new(
    "sleep::sleep",
    "Pauses execution for the given duration.",
    sleep::signature,
    sleep::analyze_sleep_sleep,
)];
