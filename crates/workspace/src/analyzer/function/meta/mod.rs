//! `meta` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

pub mod id;
pub mod tb;

/// Every `meta::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "meta::id",
        "The id part of a record id (deprecated: use `record::id`).",
        id::signature,
        id::analyze_meta_id,
    ),
    BuiltinEntry::new(
        "meta::tb",
        "The table part of a record id (deprecated: use `record::tb`).",
        tb::signature,
        tb::analyze_meta_tb,
    ),
];
