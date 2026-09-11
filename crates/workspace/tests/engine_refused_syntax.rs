//! The shapes the grammar parses *because* the engine refuses them.
//!
//! Normally the grammar is the strict one and a bad query never reaches the
//! analyzer. For a handful of forms that is the wrong trade: SurrealDB's
//! parser gives up on the whole source with a message that points at a token,
//! and an editor showing "Unexpected token `;`, expected SINCE" over a
//! collapsed file helps nobody. So the grammar accepts the shape and the
//! analyzer names the contract at the span that is actually wrong.
//!
//! That is only correct while the diagnostic really fires — a form we parse
//! and say nothing about is a query we would pass as valid and SurrealDB
//! would reject. Each case below is checked against a live SurrealDB 3.2.3,
//! and its engine error is quoted in the message or help so the user sees
//! what they will actually be told.
//!
//! These also make our grammar a strict superset of upstream
//! `surrealql-tree-sitter`, whose corpus pins all three as parseable.

mod support;

use surrealguard_diagnostics::Finding;
use surrealguard_syntax::source::SourceId;
use surrealguard_workspace::{analyze_workspace, Workspace};

const SCHEMA: &str = "\
DEFINE TABLE person SCHEMAFULL CHANGEFEED 1d;
DEFINE FIELD name ON person TYPE string;
";

/// Analyzes one query against `SCHEMA` and returns the findings on it.
fn findings(query: &str) -> Vec<Finding> {
    let mut workspace = Workspace::new(surrealguard_workspace::config::WorkspaceConfig::default());
    workspace.add_virtual_source("schema".into(), SCHEMA.into());
    let source: SourceId = workspace.add_virtual_source("query".into(), query.into());
    analyze_workspace(&workspace)
        .diagnostics
        .into_iter()
        .filter(|finding| finding.span().source() == &source)
        .collect()
}

/// The one finding with `code`, or a panic naming what was found instead.
fn only(query: &str, code: &str) -> Finding {
    let found = findings(query);
    let matching: Vec<&Finding> = found
        .iter()
        .filter(|finding| finding.code().to_string() == code)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "`{query}` should raise exactly one {code}; got {:?}",
        found
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect::<Vec<_>>()
    );
    matching[0].clone()
}

/// No finding may be a syntax error: the point of loosening the grammar is
/// that these parse.
fn assert_parses(query: &str) {
    for finding in findings(query) {
        assert!(
            !finding.code().to_string().starts_with("E0"),
            "`{query}` did not parse: {} {}",
            finding.code(),
            finding.message()
        );
    }
}

/// 3.2.3: `KILL "some-uuid-here"` is
/// "Unexpected token `a strand`, expected a UUID or a parameter".
#[test]
fn kill_with_a_plain_string_parses_and_raises_2020() {
    let query = "KILL \"some-uuid-here\";";
    assert_parses(query);
    let finding = only(query, "E2020");
    assert!(
        finding.message().contains("live-query uuid"),
        "unhelpful message: {}",
        finding.message()
    );
    let help: String = finding
        .help()
        .iter()
        .map(|help| help.message.clone())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        help.contains("expected a UUID or a parameter"),
        "help should quote the engine's own error; got {help:?}"
    );
}

/// Uuid *shaped* is still a strand to the engine — 3.2.3 rejects
/// `KILL "018e0f3a-1234-7abc-8def-0123456789ab"` the same way.
#[test]
fn kill_with_a_uuid_shaped_string_still_raises_2020() {
    let query = "KILL \"018e0f3a-1234-7abc-8def-0123456789ab\";";
    assert_parses(query);
    only(query, "E2020");
}

/// The `u'…'` literal and a parameter are what the engine takes, so neither
/// may be flagged — the near-miss half of the contract.
#[test]
fn kill_with_a_uuid_literal_or_param_is_silent() {
    for query in [
        "KILL u'018e0f3a-1234-7abc-8def-0123456789ab';",
        "LET $id = u'018e0f3a-1234-7abc-8def-0123456789ab'; KILL $id;",
    ] {
        assert_parses(query);
        let codes: Vec<String> = findings(query)
            .iter()
            .map(|finding| finding.code().to_string())
            .collect();
        assert!(
            !codes.iter().any(|code| code == "E2020"),
            "`{query}` is valid KILL but raised 2020; got {codes:?}"
        );
    }
}

/// 3.2.3: `SHOW CHANGES FOR TABLE person;` is
/// "Unexpected token `;`, expected SINCE".
#[test]
fn show_changes_without_since_parses_and_raises_2021() {
    let query = "SHOW CHANGES FOR TABLE person;";
    assert_parses(query);
    let finding = only(query, "E2021");
    assert!(
        finding.message().contains("SINCE"),
        "unhelpful message: {}",
        finding.message()
    );
    let help: String = finding
        .help()
        .iter()
        .map(|help| help.message.clone())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        help.contains("expected SINCE"),
        "help should quote the engine's own error; got {help:?}"
    );
}

/// `LIMIT` does not stand in for `SINCE`: 3.2.3 answers
/// `SHOW CHANGES FOR TABLE person LIMIT 10` with
/// "Unexpected token `LIMIT`, expected SINCE".
#[test]
fn show_changes_with_limit_but_no_since_parses_and_raises_2021() {
    let query = "SHOW CHANGES FOR TABLE person LIMIT 10;";
    assert_parses(query);
    only(query, "E2021");
}

/// The spelled-out form stays silent, so 2021 reports the missing clause and
/// not the statement.
#[test]
fn show_changes_with_since_is_silent() {
    for query in [
        "SHOW CHANGES FOR TABLE person SINCE 0;",
        "SHOW CHANGES FOR TABLE person SINCE 1 LIMIT 10;",
        "SHOW CHANGES FOR TABLE person SINCE '2024-01-01T00:00:00Z';",
    ] {
        assert_parses(query);
        let codes: Vec<String> = findings(query)
            .iter()
            .map(|finding| finding.code().to_string())
            .collect();
        assert!(
            !codes.iter().any(|code| code == "E2021"),
            "`{query}` names SINCE but raised 2021; got {codes:?}"
        );
    }
}
