//! Grammar conformance runner: feeds the two corpora extracted from
//! SurrealDB's own test suites through the tree-sitter grammar and reports
//! what does not hold.
//!
//!     cargo run -p surrealql-analyzer-syntax --example conformance
//!     cargo run -p surrealql-analyzer-syntax --example conformance -- corpus.json
//!
//! With no argument it reads the committed corpora and checks both
//! directions: every entry of the valid set (a JSON array of query strings)
//! must parse cleanly, and every entry of the rejected set (a JSON array of
//! `[query, reason]` pairs) must fail to parse. Given a path, it checks that
//! file as a valid set only — the working list when triaging a fresh
//! extraction.
//!
//! Output is one line per offender with the first ERROR/MISSING region, plus
//! a summary. `tests/conformance.rs` is the gate, this is the report, and
//! both read `tests/support` so they cannot disagree about what "fails"
//! means.

#[path = "../tests/support/mod.rs"]
mod support;

use std::fmt::Write as _;
use std::path::Path;

use surrealql_analyzer_syntax::parse::parse_source;
use surrealql_analyzer_syntax::source::SourceId;

fn main() {
    let arg = std::env::args().nth(1);
    let path = arg
        .as_deref()
        .map_or_else(support::corpus_path, |arg| Path::new(arg).to_path_buf());
    let corpus = support::load_corpus(&path);

    let failures = support::failures(&corpus);
    let mut report = String::new();
    for failure in &failures {
        let query = &corpus[failure.index];
        let context = support::preview(query, 80);
        match &failure.range {
            None => {
                let _ = writeln!(report, "#{}: parser returned no tree", failure.index);
            }
            Some(range) => {
                let snippet = support::preview(&query[range.clone()], 60);
                let _ = writeln!(
                    report,
                    "#{}: ERROR at {range:?}: `{snippet}`\n    in: {context}",
                    failure.index
                );
            }
        }
    }
    print!("{report}");
    println!(
        "---\nvalid corpus:    {}/{} parse ({} fail)",
        corpus.len() - failures.len(),
        corpus.len(),
        failures.len()
    );

    // A named corpus is a valid set only; the rejected set is committed, so
    // it is checked whenever the committed corpora are the ones being read.
    let mut accepted = 0usize;
    let rejected = if arg.is_none() {
        let rejected = support::load_rejected(&support::rejected_path());
        for (index, (query, reason)) in rejected.iter().enumerate() {
            let source = SourceId::new(format!("rejected:{index}"));
            let Ok(parsed) = parse_source(source, query.as_str()) else {
                continue;
            };
            if support::first_broken_range(parsed.tree().root_node()).is_none() {
                accepted += 1;
                println!(
                    "#{index}: ACCEPTED but must be refused: {}\n    reason: {reason}",
                    support::preview(query, 80)
                );
            }
        }
        println!(
            "rejected corpus: {}/{} refused ({accepted} wrongly accepted)",
            rejected.len() - accepted,
            rejected.len()
        );
        rejected.len()
    } else {
        0
    };
    let _ = rejected;

    if !failures.is_empty() || accepted > 0 {
        std::process::exit(1);
    }
}
