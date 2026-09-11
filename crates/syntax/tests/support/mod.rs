//! Shared plumbing for the grammar-conformance harness: loads the two
//! committed corpora — the valid set, which must parse cleanly, and the
//! rejected set, which must not parse at all — and says which entries fail to
//! parse. Used by the `conformance` integration test (the gate) and the
//! `conformance` example (the human-readable report), so the two can never
//! disagree about what "fails" means.

#![allow(dead_code)]

pub mod ast_walk;

use std::ops::Range;
use std::path::{Path, PathBuf};

use surrealguard_syntax::parse::parse_source;
use surrealguard_syntax::source::SourceId;

/// The committed valid corpus: a JSON array of query strings extracted from
/// SurrealDB's own test suites. Every entry must parse cleanly.
pub fn corpus_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/conformance_corpus.json")
}

/// The committed rejected corpus: a JSON array of `[query, reason]` pairs —
/// text that was extracted from SurrealDB's tests but is not SurrealQL (a
/// deliberate parser-error fragment, a regex assertion). Every entry must
/// fail to parse; one that starts parsing is over-acceptance.
pub fn rejected_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/conformance_rejected.json")
}

/// Every rejected entry as `(query, reason)`, in file order.
///
/// The file is a JSON array of two-element arrays, so the flat string reader
/// yields query and reason alternately; an odd count means a malformed pair
/// and is a hard error rather than a silently dropped entry.
pub fn load_rejected(path: &Path) -> Vec<(String, String)> {
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read rejected corpus {}: {error}", path.display()));
    let flat = parse_json_strings(&raw);
    assert!(
        !flat.is_empty(),
        "rejected corpus is empty at {} — the harness would vacuously pass",
        path.display()
    );
    assert!(
        flat.len().is_multiple_of(2),
        "rejected corpus {} holds {} strings — it must be `[query, reason]` pairs",
        path.display(),
        flat.len()
    );
    flat.as_chunks::<2>()
        .0
        .iter()
        .map(|[query, reason]| (query.clone(), reason.clone()))
        .collect()
}

/// Every corpus entry, in file order — the index is the entry's identity.
pub fn load_corpus(path: &Path) -> Vec<String> {
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read corpus {}: {error}", path.display()));
    let corpus = parse_json_strings(&raw);
    assert!(
        !corpus.is_empty(),
        "corpus is empty at {} — the harness would vacuously pass",
        path.display()
    );
    corpus
}

/// How one corpus entry failed: the byte range of the first `ERROR`/`MISSING`
/// node, or `None` when the parser returned no tree at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// The corpus index of the entry.
    pub index: usize,
    /// The first broken region, when a tree came back.
    pub range: Option<Range<usize>>,
}

/// Parses every entry and reports the ones that do not parse cleanly.
pub fn failures(corpus: &[String]) -> Vec<Failure> {
    corpus
        .iter()
        .enumerate()
        .filter_map(|(index, query)| {
            let source = SourceId::new(format!("corpus:{index}"));
            let Ok(parsed) = parse_source(source, query.as_str()) else {
                return Some(Failure { index, range: None });
            };
            first_broken_range(parsed.tree().root_node()).map(|range| Failure {
                index,
                range: Some(range),
            })
        })
        .collect()
}

/// The first `ERROR`/`MISSING` node's byte range, depth-first — the region
/// the report points at.
pub fn first_broken_range(node: tree_sitter::Node<'_>) -> Option<Range<usize>> {
    if node.is_error() || node.is_missing() {
        return Some(node.byte_range());
    }
    if !node.has_error() {
        return None;
    }
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();
    for child in children {
        if let Some(range) = first_broken_range(child) {
            return Some(range);
        }
    }
    Some(node.byte_range())
}

/// The first `width` characters of a query on one line, for a report.
pub fn preview(query: &str, width: usize) -> String {
    query
        .chars()
        .take(width)
        .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
        .collect()
}

/// Just enough JSON to read an array of strings without a dependency.
pub fn parse_json_strings(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut s = String::new();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => match chars.next() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('u') => {
                        let hex: String = (0..4).filter_map(|_| chars.next()).collect();
                        if let Ok(code) = u32::from_str_radix(&hex, 16) {
                            if let Some(ch) = char::from_u32(code) {
                                s.push(ch);
                            }
                        }
                    }
                    Some(other) => s.push(other),
                    None => break,
                },
                other => s.push(other),
            }
        }
        out.push(s);
    }
    out
}
