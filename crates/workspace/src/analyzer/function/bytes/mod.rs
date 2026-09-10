//! `bytes` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

pub mod len;

/// Every `bytes::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[BuiltinEntry::new(
    "bytes::len",
    "The number of bytes.",
    len::signature,
    len::analyze_bytes_len,
)];
