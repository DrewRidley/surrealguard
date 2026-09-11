//! Precision snapshot — the harness that catches *lost precision*.
//!
//! A green unit suite and a flat oracle diagnostic count both prove the same
//! thing: nothing new is reported. Neither notices a type quietly degrading
//! (`string` → `unknown`), a narrowing dying, or a response shape widening,
//! because none of those raise a finding. This test records **every** inferred
//! type the vendored corpus produces — schema field kinds, function returns,
//! per-statement response kinds, `LET`/`FOR` binding kinds, host param kinds —
//! into a committed golden file, and fails on any diff.
//!
//! The snapshot is a record of *current behaviour*, not of correct behaviour.
//! A line in it may well be wrong; the point is that it cannot change without
//! someone seeing the change.
//!
//! Regenerate with:
//!
//! ```text
//! UPDATE_SNAPSHOTS=1 cargo test -p surrealql-analyzer-workspace --test precision_snapshot
//! ```
//!
//! The snapshot pins the *answers*. It does not pin the *relation* between one
//! set of answers and the next, which is why `narrowing_floor.rs` exists beside
//! it: regenerating this file accepts any diff a reader is willing to accept,
//! including a widening, and the floor is what refuses that one.

mod support;

use std::fmt::Write as _;

use support::{analyze_corpus, diff_lines, snapshot_path, updating, Corpus};
use surrealql_analyzer_workspace::render_kind;

const SNAPSHOT: &str = "precision.snap";

const HEADER: &str = "\
# SurrealQL Analyzer precision snapshot — every inferred type the corpus produces.
#
# Regenerate: UPDATE_SNAPSHOTS=1 cargo test -p surrealql-analyzer-workspace --test precision_snapshot
#
# This file records CURRENT behaviour, not correct behaviour. A diff means a
# type changed: read it, decide whether the change is an improvement, and only
# then accept it. Silent acceptance defeats the whole harness.
";

#[test]
fn corpus_types_match_the_committed_snapshot() {
    let corpus = analyze_corpus();
    let actual = render_snapshot(&corpus);
    let path = snapshot_path(SNAPSHOT);

    if updating() {
        std::fs::create_dir_all(path.parent().expect("snapshot dir")).expect("create snapshot dir");
        std::fs::write(&path, &actual).expect("write snapshot");
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing snapshot {}\n\
             create it with: UPDATE_SNAPSHOTS=1 cargo test -p surrealql-analyzer-workspace --test precision_snapshot",
            path.display()
        )
    });

    assert!(
        expected == actual,
        "\n\
         PRECISION CHANGED — the corpus infers different types than the committed snapshot.\n\
         `-` is the recorded type, `+` is what inference produces now.\n\
         A `+ ... : unknown` or a widened kind is a precision REGRESSION; do not accept it blindly.\n\
         \n{}\n\
         Snapshot: {}\n\
         Accept with: UPDATE_SNAPSHOTS=1 cargo test -p surrealql-analyzer-workspace --test precision_snapshot\n",
        diff_lines(&expected, &actual),
        path.display()
    );
}

/// The corpus must stay all-valid: it is the *precision* harness, so any
/// finding it raises is either a false positive or a corpus bug, and either way
/// the recorded types below were inferred under error.
#[test]
fn corpus_is_free_of_error_findings() {
    let corpus = analyze_corpus();
    let errors: Vec<String> = corpus
        .analysis
        .diagnostics
        .iter()
        .filter(|finding| finding.severity() == surrealql_analyzer_diagnostics::Severity::Error)
        .map(|finding| {
            format!(
                "  {} {} at {}:{}",
                surrealql_analyzer_diagnostics::render_code(finding.code(), finding.severity()),
                finding.message(),
                finding.span().source().as_str(),
                corpus.position(finding.span())
            )
        })
        .collect();
    assert!(
        errors.is_empty(),
        "the precision corpus must be valid input, but analysis reported {} error finding(s):\n{}",
        errors.len(),
        errors.join("\n")
    );
}

fn render_snapshot(corpus: &Corpus) -> String {
    let mut out = String::from(HEADER);
    render_schema(corpus, &mut out);
    render_sources(corpus, &mut out);
    out
}

fn render_schema(corpus: &Corpus, out: &mut String) {
    let schema = &corpus.analysis.schema;

    out.push_str("\n== schema: tables ==\n");
    for (name, table) in &schema.tables {
        let mut traits = Vec::new();
        if table.schemafull {
            traits.push("schemafull".to_string());
        }
        if let Some(relation) = &table.relation {
            traits.push(format!(
                "relation {} -> {}",
                join_or_any(&relation.in_tables),
                join_or_any(&relation.out_tables)
            ));
        }
        let suffix = if traits.is_empty() {
            String::new()
        } else {
            format!("  [{}]", traits.join(", "))
        };
        let _ = writeln!(out, "table {name}{suffix}");
        for (path, field) in &table.fields {
            let mut flags = Vec::new();
            if field.computed {
                flags.push("computed");
            }
            if field.has_default {
                flags.push("default");
            }
            if field.readonly {
                flags.push("readonly");
            }
            if field.reference {
                flags.push("reference");
            }
            let flags = if flags.is_empty() {
                String::new()
            } else {
                format!("  [{}]", flags.join(", "))
            };
            let kind = field
                .kind
                .as_ref()
                .map_or_else(|| "unknown".to_string(), render_kind);
            let _ = writeln!(out, "  field {path:<28} : {kind}{flags}");
        }
    }

    out.push_str("\n== schema: functions ==\n");
    for (name, function) in &schema.functions {
        let args: Vec<String> = function
            .args
            .iter()
            .map(|arg| {
                let kind = arg
                    .kind
                    .as_ref()
                    .map_or_else(|| "unknown".to_string(), render_kind);
                format!("${}: {kind}", arg.name)
            })
            .collect();
        let (origin, kind) = match (&function.return_kind, &function.inferred_return) {
            (Some(kind), _) => ("declared", render_kind(kind)),
            (None, Some(kind)) => ("inferred", render_kind(kind)),
            (None, None) => ("none", "unknown".to_string()),
        };
        let _ = writeln!(out, "  {name}({}) -> {kind}  [{origin}]", args.join(", "));
    }

    out.push_str("\n== schema: params ==\n");
    for name in schema.params.keys() {
        let _ = writeln!(out, "  ${name}");
    }

    out.push_str("\n== schema: analyzers ==\n");
    for (name, analyzer) in &schema.analyzers {
        let _ = writeln!(
            out,
            "  {name}  tokenizers=[{}] filters=[{}]",
            analyzer.tokenizers.join(", "),
            analyzer.filters.join(", ")
        );
    }
}

fn render_sources(corpus: &Corpus, out: &mut String) {
    for (source, relative) in &corpus.sources {
        let Some(output) = corpus.analysis.sources.get(source) else {
            continue;
        };
        let _ = write!(out, "\n== source: {relative} ==\n");

        if let Some(kind) = &output.response_kind {
            let _ = writeln!(out, "  source response       : {}", render_kind(kind));
        }

        for statement in &output.statements {
            let at = corpus.position(&statement.span);
            let kind = statement
                .response_kind
                .as_ref()
                .map_or_else(|| "(no response)".to_string(), render_kind);
            let _ = writeln!(out, "  stmt {:>8} {:<16} : {kind}", at, statement.kind);
            for modifier in &statement.select_modifiers {
                let max = modifier
                    .max_len
                    .map_or_else(String::new, |max| format!(" max={max}"));
                let _ = writeln!(
                    out,
                    "       modifier {:<10} row_preserving={}{max}",
                    modifier.kind, modifier.row_preserving
                );
            }
        }

        for binding in &output.let_bindings {
            let at = corpus.position(&binding.name_span);
            let kind = binding
                .kind
                .as_ref()
                .map_or_else(|| "unknown".to_string(), render_kind);
            let name = format!("${}", binding.name);
            let _ = writeln!(out, "  let  {at:>8} {name:<16} : {kind}");
        }

        let mut params: Vec<_> = output.inferred_params.iter().collect();
        params.sort_by(|a, b| a.name.cmp(&b.name));
        for param in params {
            let kind = param
                .kind
                .as_ref()
                .map_or_else(|| "unknown".to_string(), render_kind);
            let required = if param.required {
                "required"
            } else {
                "optional"
            };
            let domain = param
                .domain
                .as_ref()
                .map_or_else(String::new, |domain| format!("  domain={domain:?}"));
            let name = format!("${}", param.name);
            let _ = writeln!(out, "  param {name:<23} : {kind}  [{required}]{domain}");
        }

        let mut findings: Vec<(u32, String)> = output
            .diagnostics
            .iter()
            .map(|finding| {
                (
                    finding.span().range().start(),
                    format!(
                        "  diag {:>8} {:<9} {:?}",
                        corpus.position(finding.span()),
                        surrealql_analyzer_diagnostics::render_code(
                            finding.code(),
                            finding.severity()
                        ),
                        finding.severity()
                    ),
                )
            })
            .collect();
        findings.sort();
        for (_, finding) in findings {
            out.push_str(&finding);
            out.push('\n');
        }
    }
}

fn join_or_any(tables: &[String]) -> String {
    if tables.is_empty() {
        "any".to_string()
    } else {
        tables.join(" | ")
    }
}
