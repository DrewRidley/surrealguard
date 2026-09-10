//! `record` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

pub mod exists;
pub mod id;
pub mod is_edge;
pub mod table;
pub mod tb;

/// Every `record::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "record::exists",
        "Whether the record exists.",
        exists::signature,
        exists::analyze_record_exists,
    ),
    BuiltinEntry::new(
        "record::id",
        "The id part of a record id.",
        id::signature,
        id::analyze_record_id,
    ),
    BuiltinEntry::new(
        "record::is_edge",
        "Whether the record is a relation edge.",
        is_edge::signature,
        is_edge::analyze_record_is_edge,
    ),
    BuiltinEntry::new(
        "record::table",
        "The table part of a record id.",
        table::signature,
        table::analyze_record_table,
    ),
    BuiltinEntry::new(
        "record::tb",
        "The table part of a record id.",
        tb::signature,
        tb::analyze_record_tb,
    ),
];
