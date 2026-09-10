//! The grammar-conformance ratchet.
//!
//! `examples/conformance_corpus.json` is known-valid SurrealQL extracted from
//! SurrealDB's own test suites. A parse error is fatal to the whole source
//! (the analyzer sees a `Partial` statement and says nothing), so every entry
//! the grammar rejects is a place the analyzer is silently wrong on valid
//! input. This test holds the set of entries that still fail against a
//! committed baseline, and it fails in BOTH directions:
//!
//! * an entry that parsed and now does not is a grammar regression;
//! * an entry that failed and now parses is progress the baseline must
//!   record — regenerate, so the win is banked and cannot be lost later.
//!
//! Regenerate with:
//!
//! ```text
//! UPDATE_SNAPSHOTS=1 cargo test -p surrealguard-syntax --test conformance
//! ```

mod support;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use support::{corpus_path, failures, load_corpus, preview};

const BASELINE: &str = "conformance_expected_failures.txt";

const HEADER: &str = "\
# Grammar-conformance baseline — the corpus entries that do NOT parse cleanly.
#
# Regenerate: UPDATE_SNAPSHOTS=1 cargo test -p surrealguard-syntax --test conformance
#
# Format:  <corpus index>  # <first 60 characters of the query>
#
# Every line is either a grammar gap still to close or a junk extraction (a
# bare `}`, a trailing `\\`, a `.*` placeholder) that is not SurrealQL at all.
# The set may only shrink: an entry listed here that starts parsing must be
# removed (regenerate), and an entry not listed here must keep parsing.
";

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(BASELINE)
}

fn updating() -> bool {
    std::env::var_os("UPDATE_SNAPSHOTS").is_some()
}

fn parse_baseline(text: &str) -> BTreeSet<usize> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let index = line.split_whitespace().next().unwrap_or_default();
            index
                .parse::<usize>()
                .unwrap_or_else(|_| panic!("malformed baseline line: `{line}`"))
        })
        .collect()
}

fn render_baseline(corpus: &[String], failing: &BTreeSet<usize>) -> String {
    let mut out = String::from(HEADER);
    let _ = writeln!(out);
    for index in failing {
        let _ = writeln!(out, "{index:>4}  # {}", preview(&corpus[*index], 60));
    }
    out
}

#[test]
fn the_corpus_parses_exactly_as_the_baseline_says() {
    let corpus = load_corpus(&corpus_path());
    let failing: BTreeSet<usize> = failures(&corpus)
        .into_iter()
        .map(|failure| failure.index)
        .collect();
    let path = baseline_path();

    if updating() {
        std::fs::write(&path, render_baseline(&corpus, &failing)).expect("write baseline");
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing baseline {}\n\
             create it with: UPDATE_SNAPSHOTS=1 cargo test -p surrealguard-syntax --test conformance",
            path.display()
        )
    });
    let expected = parse_baseline(&expected);

    let mut report = String::new();
    for index in failing.difference(&expected) {
        let _ = writeln!(
            report,
            "  REGRESSION  #{index}  parsed before, fails now: {}",
            preview(&corpus[*index], 80)
        );
    }
    for index in expected.difference(&failing) {
        let _ = writeln!(
            report,
            "  FIXED       #{index}  listed as failing, parses now: {}",
            preview(&corpus[*index], 80)
        );
    }
    assert!(
        report.is_empty(),
        "grammar conformance moved ({} of {} entries fail; baseline lists {}):\n{report}\n\
         A REGRESSION is a grammar change that broke valid SurrealQL — fix the grammar.\n\
         A FIXED entry is progress the baseline must record:\n\
         UPDATE_SNAPSHOTS=1 cargo test -p surrealguard-syntax --test conformance",
        failing.len(),
        corpus.len(),
        expected.len(),
    );
}

/// The baseline is a ratchet on real gaps only if the corpus keeps failing
/// for the reason it says: a listed entry that no longer exists (the corpus
/// shrank) would pass vacuously.
#[test]
fn every_baseline_entry_names_a_corpus_entry() {
    let corpus = load_corpus(&corpus_path());
    let expected = std::fs::read_to_string(baseline_path()).unwrap_or_default();
    for index in parse_baseline(&expected) {
        assert!(
            index < corpus.len(),
            "baseline lists #{index}, but the corpus has {} entries",
            corpus.len()
        );
    }
}
