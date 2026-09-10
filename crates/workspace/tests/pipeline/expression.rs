//! Expression typing: binary-operator operand contracts, temporal and
//! collection arithmetic, possibly-NONE operands, indexing, and casts.

use surrealguard_diagnostics::{Finding, FindingCode};
use surrealguard_workspace::{analyze_workspace, Workspace};

use crate::support::assert_no_syntax_findings;

#[test]
fn analyze_workspace_reports_binary_expression_mismatch_from_let_env() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "LET $age = 42;\nLET $bad = $age + 'x';\nRETURN $age + 'x';\nIF true { LET $score = 7; RETURN $score + 'x'; };".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    let binary_mismatches = messages
        .iter()
        .filter(|(code, message)| {
            code == "E2004" && message == "`+` can't combine a `int` and a `string`"
        })
        .count();
    assert_eq!(binary_mismatches, 3);
}

#[test]
fn analyze_workspace_accepts_temporal_and_collection_arithmetic() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE event;\nDEFINE FIELD at ON event TYPE datetime;\nDEFINE FIELD took ON event TYPE duration;\nDEFINE FIELD tags ON event TYPE array<string>;\nSELECT at + 1w AS soon, at - at AS gap, took * 2 AS twice, tags + ['x'] AS more FROM event WHERE at - took < time::now();".into(),
    );

    let output = analyze_workspace(&workspace);
    let operand_findings: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::type_error(2004))
        .collect();

    // Valid SurrealQL temporal/collection arithmetic must not be flagged.
    assert_eq!(operand_findings, Vec::<&Finding>::new());
}

#[test]
fn analyze_workspace_reports_binary_expression_type_mismatches() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD name ON person TYPE string;\nSELECT age + name AS bad FROM person;".into(),
    );

    let output = analyze_workspace(&workspace);
    let source_output = &output.sources[&source];
    let messages: Vec<_> = source_output
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    assert!(messages.iter().any(|(code, message)| {
        code == "E2004" && message == "`+` can't combine a `int` and a `string`"
    }));
}

#[test]
fn analyze_workspace_reports_index_on_non_collection() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD age ON person TYPE int;\nSELECT age[0] FROM person;".into(),
    );

    let output = analyze_workspace(&workspace);
    let diagnostics = &output.sources[&source].diagnostics;
    assert_no_syntax_findings(diagnostics);

    let messages: Vec<_> = diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::type_error(2030))
        .map(|finding| finding.message().to_string())
        .collect();
    assert_eq!(
        messages,
        vec!["a `int` can't be indexed or filtered — it is not a collection"]
    );
}

#[test]
fn analyze_workspace_reports_casts_to_unknown_types() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "RETURN <ghost> 5;\nRETURN <int> '12';".into(),
    );

    let output = analyze_workspace(&workspace);
    let diagnostics = &output.sources[&source].diagnostics;
    assert_no_syntax_findings(diagnostics);

    let messages: Vec<_> = diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::type_error(2007))
        .map(|finding| finding.message().to_string())
        .collect();
    assert_eq!(messages, vec!["`ghost` is not a known type"]);
}

#[test]
fn analyze_workspace_reports_possibly_none_operand_in_arithmetic() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD nick ON person TYPE option<string>;\nSELECT nick + 'x' FROM person;".into(),
    );

    let output = analyze_workspace(&workspace);
    let diagnostics = &output.sources[&source].diagnostics;
    assert_no_syntax_findings(diagnostics);

    assert!(
        diagnostics.iter().any(|finding| {
            finding.code() == FindingCode::type_error(2015)
                && finding.message().contains("may be NONE here")
        }),
        "expected 2015 for the option<string> operand, have: {:?}",
        diagnostics
            .iter()
            .map(|f| (f.code().to_string(), f.message().to_string()))
            .collect::<Vec<_>>()
    );
}
