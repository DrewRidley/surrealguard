//! `schema` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

pub mod table_exists;

/// Every `schema::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[BuiltinEntry::new(
    "schema::table::exists",
    "Whether a table is defined.",
    table_exists::signature,
    table_exists::analyze_schema_table_exists,
)];
