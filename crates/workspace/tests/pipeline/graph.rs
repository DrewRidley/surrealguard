//! Graph traversal and `RELATE`: edge/target table existence, relation
//! endpoint contracts, edge-filter typing, and non-relation steps.

use surrealguard_diagnostics::FindingCode;
use surrealguard_workspace::{analyze_workspace, Workspace};

use crate::support::assert_no_syntax_findings;

#[test]
fn analyze_workspace_reports_unknown_later_multi_hop_graph_edge_table() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE comment;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nSELECT * FROM person->likes->post->missing->comment;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1001))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(messages, vec!["`missing` is not a defined table"]);
}

#[test]
fn analyze_workspace_reports_unknown_graph_edge_and_target_tables() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nSELECT * FROM person->missing->post;\nSELECT * FROM person->likes->ghost;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1001))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(
        messages,
        vec![
            "`missing` is not a defined table",
            "`ghost` is not a defined table",
        ]
    );
}

#[test]
fn analyze_workspace_reports_unknown_parenthesized_graph_edge_table() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE TABLE post;\nSELECT * FROM person->(missing WHERE created_at > $since)->post;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1001))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(messages, vec!["`missing` is not a defined table"]);
}

#[test]
fn analyze_workspace_type_checks_edge_filter_conditions() {
    // Both edge-filter forms get full expression checking with the
    // edge table as the row.
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD since ON likes TYPE datetime;\nSELECT * FROM person->likes[WHERE since > 5]->post;\nSELECT * FROM person->(likes WHERE since + 1)->post;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    // One operand contract, one code — SurrealDB tolerating the
    // comparison (it kind-orders) changes nothing.
    assert!(messages.contains(&(
        "E2004".to_string(),
        "`>` can't combine a `datetime` and a `int`".to_string()
    )));
    assert!(messages.contains(&(
        "E2004".to_string(),
        "`+` can't combine a `datetime` and a `int`".to_string()
    )));
}

#[test]
fn analyze_workspace_reports_unreachable_graph_hop_targets() {
    // `likes` goes to `post`; landing on `comment` is 3003.
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE comment;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nSELECT * FROM person->likes->comment;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::graph(3002))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(
        messages,
        vec!["relation `likes` connects `person`->`likes`->`post`, so this hop cannot land on `comment`"]
    );
}

#[test]
fn analyze_workspace_reports_mismatched_graph_relation_endpoints() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nSELECT * FROM post->likes->person;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::graph(3002))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(
        messages,
        vec!["relation `likes` connects `person`->`likes`->`post`, but this step traverses `->` from `post`"]
    );
}

#[test]
fn analyze_workspace_allows_matching_relate_relation_endpoints() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nRELATE person:one->likes->post:one;".into(),
    );

    let output = analyze_workspace(&workspace);

    assert!(output
        .diagnostics
        .iter()
        .all(|finding| finding.code() != FindingCode::graph(3002)));
}

#[test]
fn analyze_workspace_reports_mismatched_relate_relation_endpoints() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nRELATE post:one->likes->person:one;".into(),
    );

    let output = analyze_workspace(&workspace);
    let mismatches: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::graph(3002))
        .collect();

    let messages: Vec<_> = mismatches
        .iter()
        .map(|finding| finding.message().to_string())
        .collect();
    // One finding comparing the declared shape against the written one.
    assert_eq!(
        messages,
        vec![
            "relation `likes` connects `person`->`likes`->`post`, but this RELATE writes `post`->`likes`->`person`",
        ]
    );
    assert_eq!(mismatches[0].span().source(), &source);
    assert_eq!(output.sources[&source].diagnostics.len(), 1);
}

#[test]
fn analyze_workspace_reports_unknown_relate_edge_and_target_tables() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE TABLE post;\nRELATE person:one->missing->post:one;\nRELATE person:one->likes->ghost:one;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| matches!(finding.code(), code if code == FindingCode::schema(1001) || code == FindingCode::from_number(1001)))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(
        messages,
        vec![
            "`missing` is not a defined table",
            "`ghost` is not a defined table",
            "`likes` is not a defined table",
        ]
    );
}

#[test]
fn analyze_workspace_validates_parenthesized_graph_where_against_edge_fields() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD created_at ON likes TYPE datetime;\nSELECT * FROM person->(likes WHERE created_at > $since)->post;\nSELECT * FROM person->(likes WHERE missing_since > $since)->post;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1002))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(messages, vec!["`likes` has no field `missing_since`"]);
}

#[test]
fn analyze_workspace_validates_bracketed_graph_filter_against_edge_fields() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD created_at ON likes TYPE datetime;\nSELECT * FROM person->likes[WHERE created_at > $since]->post;\nSELECT * FROM person->likes[WHERE missing_since > $since]->post;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1002))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(messages, vec!["`likes` has no field `missing_since`"]);
}

#[test]
fn analyze_workspace_reports_graph_step_through_non_relation_table() {
    let mut workspace = Workspace::default();
    // A multi-target step `->(likes, post)` requires every named edge to
    // be a relation table; `post` is a plain table, so it trips 3001.
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nSELECT ->(likes, post) FROM person;".into(),
    );

    let output = analyze_workspace(&workspace);
    let diagnostics = &output.sources[&source].diagnostics;
    assert_no_syntax_findings(diagnostics);

    let messages: Vec<_> = diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::graph(3001))
        .map(|finding| finding.message().to_string())
        .collect();
    assert_eq!(
        messages,
        vec!["`post` can't be traversed — it is not a relation table"]
    );
}

#[test]
fn analyze_workspace_reports_single_step_traversal_through_plain_table() {
    let mut workspace = Workspace::default();
    // With no edge to land from, a plain table in step position is a
    // traversal, and a traversal must name a relation. The valid hop
    // form `->likes->post` stays quiet.
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nSELECT ->post FROM person;\nSELECT ->likes->post FROM person;".into(),
    );

    let output = analyze_workspace(&workspace);
    let diagnostics = &output.sources[&source].diagnostics;
    assert_no_syntax_findings(diagnostics);

    let messages: Vec<_> = diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::graph(3001))
        .map(|finding| finding.message().to_string())
        .collect();
    assert_eq!(
        messages,
        vec!["`post` can't be traversed — it is not a relation table"]
    );
}
