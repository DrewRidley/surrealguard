//! `sequence` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

pub mod next;
pub mod nextval;

/// Every `sequence::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "sequence::next",
        "The next value of a sequence (alias of `sequence::nextval`).",
        next::signature,
        next::analyze_sequence_next,
    ),
    BuiltinEntry::new(
        "sequence::nextval",
        "The next value of a sequence.",
        nextval::signature,
        nextval::analyze_sequence_nextval,
    ),
];
