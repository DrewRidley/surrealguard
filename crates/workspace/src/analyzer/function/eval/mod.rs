//! `eval` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

pub mod gql;
pub mod surql;

/// Every `eval::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "eval::gql",
        "Evaluates a GraphQL query string against the database.",
        gql::signature,
        gql::analyze_eval_gql,
    ),
    BuiltinEntry::new(
        "eval::surql",
        "Evaluates a SurrealQL string, with optional bindings.",
        surql::signature,
        surql::analyze_eval_surql,
    ),
];
