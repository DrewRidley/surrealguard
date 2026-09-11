//! Measurement hook (ignored by default): how whole-document analysis cost
//! scales with document size.
//!
//! The LSP re-runs analysis on every keystroke, so the cost of one
//! `didChange` is the cost of `analyze_workspace` over the edited document
//! plus its schema. An editor budget is per-*document*, not per-statement, so
//! the number that matters is **microseconds per line**: if it is flat the
//! engine is linear in document size and a big file is merely proportionally
//! slower; if it climbs the engine is superlinear and a big file is unusable.
//!
//! This runs entirely in-process — no stdio, no JSON, no LSP — so a climbing
//! per-line cost here localises the superlinearity to `crates/workspace`.
//!
//! Run with:
//!   cargo test -p surrealguard-workspace --test `analysis_scaling` -- --ignored --nocapture
//!
//! The target that matters: the SurrealDB `surrealql-language-server`
//! integration gates CI on whole-document analysis under **60 ms for a
//! 3,200-line file**.

use std::time::Instant;

use surrealguard_workspace::{analyze_workspace, Workspace};

/// A small schema the generated document types against — the same shape
/// `scratchpad/posbench.py` uses over the wire, so the in-process and
/// end-to-end numbers describe the same work.
const SCHEMA: &str = "DEFINE TABLE person SCHEMAFULL;\n\
                      DEFINE FIELD name ON person TYPE string;\n\
                      DEFINE FIELD age ON person TYPE int;\n\
                      DEFINE FIELD email ON person TYPE option<string>;\n\
                      DEFINE TABLE post SCHEMAFULL;\n\
                      DEFINE FIELD title ON post TYPE string;\n\
                      DEFINE FIELD author ON post TYPE record<person>;\n";

/// A document of `lines` statements, each an unused `LET` (one finding per
/// line) holding a SELECT over the schema above.
fn query_doc(lines: usize) -> String {
    let mut out = String::new();
    for i in 0..lines {
        out.push_str(&format!(
            "LET $row{i} = (SELECT name, age, bogus{i} FROM person \
             WHERE age > {} AND name != 'x{i}');\n",
            i % 90
        ));
    }
    out
}

/// The same document with every statement well-formed, so the curve can be
/// read with the finding count held at zero — a superlinearity that survives
/// this is not in finding assembly.
fn clean_doc(lines: usize) -> String {
    let mut out = String::new();
    for i in 0..lines {
        out.push_str(&format!(
            "LET $row{i} = (SELECT name, age FROM person \
             WHERE age > {} AND name != 'x{i}');\nRETURN $row{i};\n",
            i % 90
        ));
    }
    out
}

fn measure(label: &str, docs: &[(usize, String)]) {
    eprintln!("\n== {label} ==");
    eprintln!(
        "{:>6} {:>8} {:>7} {:>10} {:>10}",
        "lines", "KB", "diags", "total ms", "us/line"
    );
    for (lines, text) in docs {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source("schema".into(), SCHEMA.into());
        workspace.add_virtual_source("queries".into(), text.clone());

        // One warm pass, then the measured one; a single pass at 3,200 lines
        // dwarfs any noise, so one sample is enough to read the shape.
        let warm = analyze_workspace(&workspace);
        let diags = warm.diagnostics.len();

        let start = Instant::now();
        let _ = analyze_workspace(&workspace);
        let elapsed = start.elapsed();

        eprintln!(
            "{:>6} {:>8.1} {:>7} {:>10.1} {:>10.1}",
            lines,
            text.len() as f64 / 1024.0,
            diags,
            elapsed.as_secs_f64() * 1000.0,
            elapsed.as_secs_f64() * 1e6 / *lines as f64,
        );
    }
}

#[test]
#[ignore]
fn analysis_cost_per_line_by_document_size() {
    let sizes = [200usize, 400, 800, 1600, 3200];

    // Warm the process (allocator arenas, the parser's tables) so the first
    // size in the table is not reading a cost the others never pay.
    let mut warmup = Workspace::default();
    warmup.add_virtual_source("schema".into(), SCHEMA.into());
    warmup.add_virtual_source("queries".into(), query_doc(400));
    let _ = analyze_workspace(&warmup);

    let dirty: Vec<(usize, String)> = sizes.iter().map(|n| (*n, query_doc(*n))).collect();
    measure("one finding per line", &dirty);

    let clean: Vec<(usize, String)> = sizes.iter().map(|n| (*n, clean_doc(*n))).collect();
    measure("zero findings", &clean);
}
