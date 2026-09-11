//! The grammar-conformance gate.
//!
//! Two corpora, both extracted from SurrealDB's own test suites, both held at
//! 100%:
//!
//! * `examples/conformance_corpus.json` — the **valid** set. Known-good
//!   SurrealQL that the grammar must parse cleanly. A parse error is fatal to
//!   the whole source (the analyzer sees a `Partial` statement and says
//!   nothing), so an entry the grammar rejects is a place the analyzer is
//!   silently wrong on valid input.
//! * `examples/conformance_rejected.json` — the **rejected** set, as
//!   `[query, reason]` pairs. Text that came out of the same test suites but
//!   is not SurrealQL: deliberate fragments from SurrealDB's parser
//!   error-handling tests (`}`, `SELECT * FROM`), and regex assertions on
//!   `INFO` output (`PASSHASH .*`). The grammar must refuse every one.
//!
//! So the gate fails in both directions:
//!
//! * a valid entry that stops parsing is a grammar regression — fix the
//!   grammar, never the corpus;
//! * a rejected entry that starts parsing is over-acceptance — the grammar
//!   grew looser than the language, and a construct the engine refuses would
//!   now reach the analyzer as if it were real.
//!
//! There is no baseline file and no `UPDATE_SNAPSHOTS` path: at 100% in both
//! directions the invariant is absolute, so there is nothing to re-record.
//! An entry that cannot be made to hold belongs in the other corpus with a
//! reason, and that move is a deliberate edit, not a regeneration.
//!
//! The human-readable report is:
//!
//! ```text
//! cargo run -p surrealguard-syntax --example conformance
//! ```

mod support;

use std::fmt::Write as _;

use support::{
    corpus_path, failures, first_broken_range, load_corpus, load_rejected, preview, rejected_path,
};

use surrealguard_syntax::parse::parse_source;
use surrealguard_syntax::source::SourceId;

#[test]
fn every_valid_corpus_entry_parses_cleanly() {
    let corpus = load_corpus(&corpus_path());
    let failures = failures(&corpus);

    let mut report = String::new();
    for failure in &failures {
        let query = &corpus[failure.index];
        match &failure.range {
            None => {
                let _ = writeln!(
                    report,
                    "  #{}  parser returned no tree: {}",
                    failure.index,
                    preview(query, 80)
                );
            }
            Some(range) => {
                let _ = writeln!(
                    report,
                    "  #{}  ERROR at {range:?} `{}`\n       in: {}",
                    failure.index,
                    preview(&query[range.clone()], 60),
                    preview(query, 80)
                );
            }
        }
    }

    assert!(
        report.is_empty(),
        "{} of {} valid corpus entries do not parse:\n{report}\n\
         These are known-valid SurrealQL from SurrealDB's own tests. Fix the\n\
         grammar (crates/tree-sitter-surrealql/grammar.js, then regenerate);\n\
         do not edit the corpus to make this pass. If an entry turns out not\n\
         to be SurrealQL at all, move it to examples/conformance_rejected.json\n\
         with a reason.",
        failures.len(),
        corpus.len(),
    );
}

#[test]
fn every_rejected_corpus_entry_is_refused() {
    let rejected = load_rejected(&rejected_path());

    let mut report = String::new();
    for (index, (query, reason)) in rejected.iter().enumerate() {
        let source = SourceId::new(format!("rejected:{index}"));
        let parsed = match parse_source(source, query.as_str()) {
            Ok(parsed) => parsed,
            // No tree at all is a refusal, which is what we want.
            Err(_) => continue,
        };
        if first_broken_range(parsed.tree().root_node()).is_none() {
            let _ = writeln!(
                report,
                "  #{index}  parses now, but must not: {}\n       reason: {reason}",
                preview(query, 80)
            );
        }
    }

    assert!(
        report.is_empty(),
        "the grammar accepts {} entr{} it must refuse:\n{report}\n\
         This is over-acceptance: the grammar grew looser than the language,\n\
         so text SurrealDB itself refuses would reach the analyzer as if it\n\
         were a real query. Tighten the grammar. Only move an entry to\n\
         examples/conformance_corpus.json if it is genuinely valid SurrealQL\n\
         — verify on a live engine first.",
        report.lines().count() / 2,
        if report.lines().count() / 2 == 1 {
            "y"
        } else {
            "ies"
        },
    );
}

/// A rejected entry earns its place by saying why it is not SurrealQL. An
/// empty reason turns the set into an unexplained denylist, which is exactly
/// the shape the diagnostics design forbids.
#[test]
fn every_rejected_corpus_entry_carries_a_reason() {
    for (index, (query, reason)) in load_rejected(&rejected_path()).iter().enumerate() {
        assert!(
            !reason.trim().is_empty(),
            "rejected entry #{index} ({}) has no reason",
            preview(query, 60)
        );
    }
}

/// The two corpora are disjoint: the same text cannot be required to parse
/// and required to fail.
#[test]
fn the_two_corpora_do_not_overlap() {
    let corpus = load_corpus(&corpus_path());
    for (query, _) in load_rejected(&rejected_path()) {
        assert!(
            !corpus.contains(&query),
            "`{}` is in both the valid and the rejected corpus",
            preview(&query, 60)
        );
    }
}
