//! The negative corpus — the harness that catches a check that *stopped
//! firing*.
//!
//! Three harnesses already watch the all-valid corpus, and every one of them
//! is blind in the same direction. `precision.snap` records inferred types, so
//! a diagnostic that vanishes moves nothing. `any_baseline.txt` counts `Any`
//! leaves. The oracle reads a real-world corpus that is *mostly* valid, so a
//! finding disappearing there is ambiguous — it may be a fix.
//!
//! This one is unambiguous: every file under `tests/corpus/invalid/` is wrong
//! on purpose, and the golden file records what we say about it. A finding
//! that disappears is a failure, not a diff to accept.
//!
//! **Codes and positions only, never message text.** Message wording is
//! rendering's business and will move; a golden that pins prose is a golden
//! nobody dares regenerate, and one nobody dares regenerate is one that gets
//! regenerated blind. Line and code are the contract: *this line is wrong, and
//! this is what we call it*.
//!
//! Regenerate with:
//!
//! ```text
//! UPDATE_SNAPSHOTS=1 cargo test -p surrealguard-workspace --test negative_corpus
//! ```

mod support;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use surrealguard_workspace::{analyze_workspace, Workspace};

use support::{diff_lines, snapshot_path, updating};

const GOLDEN: &str = "invalid.snap";

const HEADER: &str = "\
# SurrealGuard negative corpus — every finding the INVALID corpus produces.
#
# Regenerate: UPDATE_SNAPSHOTS=1 cargo test -p surrealguard-workspace --test negative_corpus
#
# Every file under tests/corpus/invalid/ is wrong on purpose. This file records
# `file:line  CODE` for each finding — never the message, which is rendering's
# business and will move.
#
# Its ratchet runs opposite to the oracle's: a finding that DISAPPEARS is a
# check that stopped firing, and that is a regression until someone argues
# otherwise.
";

#[test]
fn the_invalid_corpus_still_reports_everything_it_used_to() {
    let actual = render();
    let path = snapshot_path(GOLDEN);

    if updating() {
        std::fs::create_dir_all(path.parent().expect("snapshot dir")).expect("create snapshot dir");
        std::fs::write(&path, &actual).expect("write golden");
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing golden {}\n\
             create it with: UPDATE_SNAPSHOTS=1 cargo test -p surrealguard-workspace --test negative_corpus",
            path.display()
        )
    });
    assert_eq!(
        expected,
        actual,
        "the invalid corpus reports something different.\n\n{}\n\n\
         A REMOVED line is a check that stopped firing — the reason this file exists.\n\
         Accept with: UPDATE_SNAPSHOTS=1 cargo test -p surrealguard-workspace --test negative_corpus",
        diff_lines(&expected, &actual)
    );
}

/// Every invalid-corpus file must actually be invalid. A fixture that reports
/// nothing pins nothing, and would sit here looking like coverage.
#[test]
fn every_invalid_fixture_reports_something() {
    let (workspace, sources) = analyzed();
    let analysis = analyze_workspace(&workspace);
    let silent: Vec<&String> = sources
        .iter()
        .filter(|(id, _)| {
            analysis
                .sources
                .get(id)
                .is_none_or(|source| source.diagnostics.is_empty())
        })
        .map(|(_, relative)| relative)
        .collect();
    assert!(
        silent.is_empty(),
        "these fixtures report nothing, so they pin nothing: {silent:?}"
    );
}

fn invalid_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/invalid")
}

/// The invalid corpus registered into one workspace.
///
/// Each fixture is analyzed against the *same* schema half of the valid
/// corpus, so a fixture can be a wrong query about a real table without
/// restating the schema.
fn analyzed() -> (
    Workspace,
    Vec<(surrealguard_syntax::source::SourceId, String)>,
) {
    let mut workspace = Workspace::default();
    let mut sources = Vec::new();
    for (relative, text) in support::corpus_files() {
        if !relative.starts_with("schema/") {
            continue;
        }
        workspace.add_file_source(PathBuf::from(&relative), text);
    }
    let root = invalid_root();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap_or_else(|error| panic!("read {}: {error}", root.display()))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "surql"))
        .collect();
    paths.sort();
    assert!(
        !paths.is_empty(),
        "the invalid corpus is empty at {} — this harness would vacuously pass",
        root.display()
    );
    for path in paths {
        let relative = format!(
            "invalid/{}",
            path.file_name().expect("a file name").to_string_lossy()
        );
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let id = workspace.add_file_source(PathBuf::from(&relative), text);
        sources.push((id, relative));
    }
    (workspace, sources)
}

fn render() -> String {
    let (workspace, sources) = analyzed();
    let analysis = analyze_workspace(&workspace);
    let mut out = String::from(HEADER);
    for (id, relative) in &sources {
        let _ = write!(out, "\n== {relative} ==\n");
        let Some(source) = analysis.sources.get(id) else {
            continue;
        };
        let index = workspace.registry().line_index(id);
        let mut rows: Vec<String> = source
            .diagnostics
            .iter()
            .map(|finding| {
                let line = index.map_or(0, |index| {
                    index.line_column(finding.span().range().start()).line + 1
                });
                format!("  {line:>4}  {}", finding.code())
            })
            .collect();
        rows.sort();
        for row in rows {
            let _ = writeln!(out, "{row}");
        }
    }
    out
}
