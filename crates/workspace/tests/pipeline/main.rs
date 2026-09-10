//! Pipeline tests: the analyzer driven end to end through the public
//! `analyze_query` / `analyze_workspace` API, grouped by what they exercise.
//! These used to live inline in `crates/workspace/src/analysis.rs`.

#[path = "../support/mod.rs"]
mod support;

mod expression;
mod flow;
mod functions;
mod graph;
mod incremental;
mod mutation;
mod params;
mod pipeline;
mod schema;
mod select;
