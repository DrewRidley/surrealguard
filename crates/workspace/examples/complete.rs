//! Completion probe: rank completions for a cursor against a real workspace.
//!
//! ```text
//! cargo run --example complete -- <dir-of-surql> "SELECT ▏ FROM organization"
//! ```
//!
//! Every `.surql` file under `<dir-of-surql>` is loaded as schema, the query
//! is analyzed against it, and the ranked candidate list at `▏` is printed.
//! This is the same path the language server takes, so what it prints is what
//! an editor shows.

use surrealguard_syntax::parse::parse_source;
use surrealguard_workspace::analysis::{analyze_workspace, Workspace};
use surrealguard_workspace::{complete_at, completion_context_at};

fn main() {
    let mut args = std::env::args().skip(1);
    let root = args
        .next()
        .expect("usage: complete <dir> \"<query with ▏>\"");
    let query = args
        .next()
        .expect("usage: complete <dir> \"<query with ▏>\"");
    let limit: usize = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20);

    let offset = query.find('▏').expect("place the cursor with `▏`") as u32;
    let query = query.replace('▏', "");

    let mut workspace = Workspace::default();
    let mut files: Vec<std::path::PathBuf> = walk(std::path::Path::new(&root));
    files.sort();
    for path in files {
        if let Ok(text) = std::fs::read_to_string(&path) {
            workspace.add_file_source(path, text);
        }
    }
    let query_id = workspace.add_virtual_source("probe".into(), query.clone());
    let analysis = analyze_workspace(&workspace);
    let output = analysis.sources.get(&query_id).cloned().unwrap_or_default();
    let parsed = parse_source(query_id, query).expect("the probe query parses");

    let context = completion_context_at(&output, &analysis.schema, &parsed, offset);
    println!(
        "context={:?} tables={:?} expected={:?} prefix={:?}",
        context.kind,
        context.tables,
        context
            .expected
            .as_ref()
            .map(surrealguard_workspace::render_kind),
        context.prefix,
    );
    // Completion runs per keystroke, so its own cost is worth showing.
    let started = std::time::Instant::now();
    let candidates = complete_at(&output, &analysis.schema, &parsed, offset);
    println!(
        "{} candidates in {:.3}ms",
        candidates.len(),
        started.elapsed().as_secs_f64() * 1000.0
    );

    for (rank, candidate) in candidates.into_iter().take(limit).enumerate() {
        println!(
            "{rank:>3}  {:<9?} {:<30} {:<44} {:.3}",
            candidate.kind,
            candidate.label,
            candidate.detail.unwrap_or_default(),
            candidate.score,
        );
    }
}

/// Every `.surql` file under `root`, recursively.
fn walk(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else if path.extension().is_some_and(|ext| ext == "surql") {
            out.push(path);
        }
    }
    out
}
