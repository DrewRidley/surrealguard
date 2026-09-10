//! `search` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

pub mod analyze;
pub mod highlight;
pub mod linear;
pub mod offsets;
pub mod rrf;
pub mod score;

/// Every `search::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "search::analyze",
        "The tokens an analyzer produces for a string.",
        analyze::signature,
        analyze::analyze_search_analyze,
    ),
    BuiltinEntry::new(
        "search::highlight",
        "The matched terms of a full-text `@@` match, wrapped in the given markers.",
        highlight::signature,
        highlight::analyze_search_highlight,
    ),
    BuiltinEntry::new(
        "search::linear",
        "Reranks search results by a linear combination of scores.",
        linear::signature,
        linear::analyze_search_linear,
    ),
    BuiltinEntry::new(
        "search::offsets",
        "The byte offsets of the terms matched by a full-text `@@` match.",
        offsets::signature,
        offsets::analyze_search_offsets,
    ),
    BuiltinEntry::new(
        "search::rrf",
        "Fuses ranked result lists with reciprocal rank fusion.",
        rrf::signature,
        rrf::analyze_search_rrf,
    ),
    BuiltinEntry::new(
        "search::score",
        "The relevance score of a full-text `@@` match.",
        score::signature,
        score::analyze_search_score,
    ),
];
