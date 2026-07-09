//! Grammar conformance runner: feeds a corpus of known-valid SurrealQL
//! (extracted from SurrealDB's own test suites) through the tree-sitter
//! grammar and reports anything that fails to parse cleanly.
//!
//!     cargo run -p surrealguard-syntax --example conformance -- corpus.json
//!
//! The corpus file is a JSON array of query strings. Output is one line
//! per failing entry with the first ERROR/MISSING region, plus a summary —
//! the working list for the grammar fork.

use std::fmt::Write as _;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: conformance <corpus.json>");
    let raw = std::fs::read_to_string(&path).expect("corpus file readable");
    let corpus: Vec<String> = parse_json_strings(&raw);

    let mut failures = 0usize;
    let mut report = String::new();
    for (index, query) in corpus.iter().enumerate() {
        let source = surrealguard_syntax::source::SourceId::new(format!("corpus:{index}"));
        let Ok(parsed) = surrealguard_syntax::parse::parse_source(source, query.as_str()) else {
            failures += 1;
            let _ = writeln!(report, "#{index}: parser returned no tree");
            continue;
        };
        if let Some(range) = first_broken_range(parsed.tree().root_node()) {
            failures += 1;
            let snippet: String = query[range.clone()].chars().take(60).collect();
            let context: String = query.chars().take(80).collect();
            let _ = writeln!(
                report,
                "#{index}: ERROR at {range:?}: `{}`\n    in: {}",
                snippet.replace('\n', " "),
                context.replace('\n', " ")
            );
        }
    }

    print!("{report}");
    println!(
        "---\n{failures}/{} corpus entries fail to parse",
        corpus.len()
    );
    if failures > 0 {
        std::process::exit(1);
    }
}

fn first_broken_range(node: tree_sitter::Node<'_>) -> Option<std::ops::Range<usize>> {
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

/// Just enough JSON to read an array of strings without a dependency.
fn parse_json_strings(raw: &str) -> Vec<String> {
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
