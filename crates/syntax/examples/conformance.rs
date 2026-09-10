//! Grammar conformance runner: feeds a corpus of known-valid SurrealQL
//! (extracted from SurrealDB's own test suites) through the tree-sitter
//! grammar and reports anything that fails to parse cleanly.
//!
//!     cargo run -p surrealguard-syntax --example conformance -- corpus.json
//!
//! The corpus file is a JSON array of query strings. Output is one line
//! per failing entry with the first ERROR/MISSING region, plus a summary —
//! the working list for the grammar fork. The committed corpus is also held
//! against a baseline by `tests/conformance.rs`; this example is the report,
//! that test is the gate, and both read `tests/support`.

#[path = "../tests/support/mod.rs"]
mod support;

use std::fmt::Write as _;
use std::path::Path;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: conformance <corpus.json>");
    let corpus = support::load_corpus(Path::new(&path));

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
        "---\n{}/{} corpus entries fail to parse",
        failures.len(),
        corpus.len()
    );
    if !failures.is_empty() {
        std::process::exit(1);
    }
}
