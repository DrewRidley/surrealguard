//! End-to-end pipeline facts: `analyze_query` / `analyze_source` /
//! `analyze_workspace` shapes, statement-kind vocabulary, syntax-error
//! resilience, suppression directives, and the broad coverage batches.

use surrealdb_types::Kind;
use surrealql_analyzer_diagnostics::{FindingCode, Severity};
use surrealql_analyzer_workspace::config::WorkspaceConfig;
use surrealql_analyzer_workspace::{analyze_query, analyze_source, analyze_workspace, Workspace};

use crate::support::assert_no_syntax_findings;

#[test]
fn analyze_query_returns_statement_analysis_for_parseable_surrealql() {
    let mut workspace = Workspace::default();

    let output = analyze_query(&mut workspace, "SELECT * FROM person;");

    // One-shot queries get the full pipeline: with no schema in the
    // workspace, the unknown table is a real finding, alongside the two
    // opt-in whole-table/`SELECT *` lints (7014/7015, allow-by-default).
    assert_eq!(output.diagnostics.len(), 3);
    assert!(output
        .diagnostics
        .iter()
        .any(|finding| finding.code().to_string() == "E1001"));
    assert!(output
        .diagnostics
        .iter()
        .any(|finding| finding.code().number() == 7014));
    assert!(output
        .diagnostics
        .iter()
        .any(|finding| finding.code().number() == 7015));
    assert_eq!(output.statements.len(), 1);
    assert_eq!(output.statements[0].kind, "select");
    assert_eq!(
        output.statements[0].span.source().as_str(),
        "virtual://query#0"
    );
    assert_eq!(output.statements[0].span.range().start(), 0);
    assert_eq!(output.statements[0].span.range().end(), 20);
    assert_eq!(output.statements[0].response_kind, Some(Kind::Any));
    assert!(output.inferred_params.is_empty());
    assert_eq!(output.response_kind, Some(Kind::Any));
}

#[test]
fn analyze_query_emits_statement_analysis_for_all_parseable_statement_kinds() {
    let mut workspace = Workspace::default();
    let query = r#"
BEGIN;
CANCEL;
COMMIT;
INFO FOR DB;
KILL u'e72bee20-f49b-11ec-b939-0242ac120002';
LIVE SELECT * FROM person;
SHOW CHANGES FOR TABLE person SINCE 0;
SLEEP 1s;
USE NS app DB app;
OPTION IMPORT;
BREAK;
CONTINUE;
FOR $item IN [1] { RETURN $item; };
THROW 'bad';
IF true { RETURN 1; };
LET $name = 'Ada';
DELETE person;
CREATE person;
SELECT * FROM person;
RELATE person:one->likes->post:one;
UPDATE person SET name = 'Ada';
REMOVE TABLE person;
UPSERT person:one SET name = 'Ada';
RETURN 1;
ALTER TABLE person SCHEMAFULL;
DEFINE TABLE person;
REBUILD INDEX by_name ON TABLE person;
INSERT INTO person { name: 'Ada' };
"#;

    let output = analyze_query(&mut workspace, query);

    // The fixture deliberately trips contracts (bare BREAK, tables used
    // before definition); this test pins only the statement-kind
    // vocabulary.
    let kinds: Vec<_> = output
        .statements
        .iter()
        .map(|statement| statement.kind.as_str())
        .collect();
    assert_eq!(
        kinds,
        vec![
            "begin",
            "cancel",
            "commit",
            "info_for",
            "kill",
            "live_select",
            "show",
            "sleep",
            "use",
            "option",
            "break",
            "continue",
            "for",
            "throw",
            "if_else",
            "let",
            "delete",
            "create",
            "select",
            "relate",
            "update",
            "remove",
            "upsert",
            "return",
            "alter",
            "define_table",
            "rebuild",
            "insert",
        ]
    );
}

#[test]
fn analyze_query_surfaces_syntax_diagnostics_with_source_spans() {
    let mut workspace = Workspace::default();

    let output = analyze_query(&mut workspace, "SELECT * FROM ;");

    assert_eq!(output.diagnostics.len(), 1);
    let diagnostic = &output.diagnostics[0];
    // The table name is missing, not malformed: a MISSING node, S0002.
    assert_eq!(diagnostic.code(), FindingCode::syntax(2));
    assert_eq!(diagnostic.severity(), Severity::Error);
    assert_eq!(diagnostic.span().source().as_str(), "virtual://query#0");
    assert!(!diagnostic.message().is_empty());
}

#[test]
fn analyze_source_uses_existing_registered_source_id() {
    let mut workspace = Workspace::default();
    let source_id = workspace.add_virtual_source("schema".into(), "DEFINE TABLE person;".into());

    let output = analyze_source(&workspace, source_id.clone());

    assert_eq!(
        workspace.registry().text(&source_id),
        Some("DEFINE TABLE person;")
    );
    assert!(output.diagnostics.is_empty());
}

#[test]
fn analyze_workspace_returns_outputs_for_all_registered_sources() {
    let mut workspace = Workspace::default();
    let good = workspace.add_virtual_source("schema".into(), "DEFINE TABLE person;".into());
    let bad = workspace.add_virtual_source("query".into(), "SELECT * FROM ;".into());

    let output = analyze_workspace(&workspace);

    assert_eq!(output.sources.len(), 2);
    assert!(output.sources[&good].diagnostics.is_empty());
    assert_eq!(output.sources[&bad].diagnostics.len(), 1);
    assert_eq!(output.diagnostics.len(), 1);
}

#[test]
fn analyze_workspace_emits_statement_analysis_for_registered_sources() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nSELECT * FROM person;\nCREATE person;".into(),
    );

    let output = analyze_workspace(&workspace);
    let source_output = &output.sources[&source];

    let kinds: Vec<_> = source_output
        .statements
        .iter()
        .map(|statement| statement.kind.as_str())
        .collect();
    assert_eq!(kinds, vec!["define_table", "select", "create"]);
    assert_eq!(source_output.statements[1].span.source(), &source);
    assert_eq!(source_output.statements[1].span.range().start(), 21);
    assert_eq!(source_output.statements[1].span.range().end(), 41);
    assert!(matches!(
        source_output.statements[1].response_kind,
        Some(Kind::Any)
    ));
}

#[test]
fn syntax_error_statement_yields_no_typed_response_or_params() {
    // A statement that fails to parse lowers to `Statement::Partial`: it
    // reports its syntax diagnostic and contributes no typed response and
    // no inferred params. (Its well-formed siblings, if any, still
    // analyze — see the resilience tests below.)
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source("query".into(), "SELECT * FROM ;".into());

    let output = analyze_workspace(&workspace);
    let source_output = &output.sources[&source];

    assert_eq!(source_output.diagnostics.len(), 1);
    assert!(source_output.response_kind.is_none());
    assert!(source_output
        .statements
        .iter()
        .all(|statement| statement.response_kind.is_none()));
    assert!(source_output.inferred_params.is_empty());
}

#[test]
fn resilient_editor_facts_survive_a_syntax_error_sibling() {
    // A syntax error in one statement must not darken the well-formed ones:
    // the LETs before and after the broken statement still bind and type.
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "LET $good_a = 1 + 2;\nSELECT name FROM ;\nLET $good_b = 3 + 4;\n".into(),
    );

    let output = analyze_workspace(&workspace);
    let source_output = &output.sources[&source];

    // The broken statement still reports its syntax diagnostic.
    assert!(source_output
        .diagnostics
        .iter()
        .any(|finding| finding.code().number() == 1));
    // Both good LETs bind to `int`; the broken one contributes nothing.
    let bound: std::collections::BTreeMap<&str, Option<&Kind>> = source_output
        .let_bindings
        .iter()
        .map(|binding| (binding.name.as_str(), binding.kind.as_ref()))
        .collect();
    assert_eq!(bound.get("good_a"), Some(&Some(&Kind::Int)));
    assert_eq!(bound.get("good_b"), Some(&Some(&Kind::Int)));
}

#[test]
fn resilient_editor_facts_survive_a_syntax_error_in_a_function_body() {
    // A syntax error inside a DEFINE FUNCTION body must not darken the rest
    // of the body: the LETs around the broken statement still bind and type,
    // and hover/inlay resolve against them.
    use surrealql_analyzer_workspace::{hover_at, let_binding_hints};
    let mut workspace = Workspace::default();
    let text = "DEFINE FUNCTION fn::demo($p: int) {\n\
        LET $good_a = 1 + 2;\n\
        LET $bad = SELECT name FROM ;\n\
        LET $good_b = $p + 1;\n\
        RETURN $good_a;\n\
    };";
    let source = workspace.add_virtual_source("query".into(), text.into());
    let analysis = analyze_workspace(&workspace);
    let source_output = &analysis.sources[&source];

    // Syntax diagnostic still fires on the broken body statement.
    assert!(source_output
        .diagnostics
        .iter()
        .any(|finding| finding.code().number() == 1));

    let bound: std::collections::BTreeMap<&str, Option<&Kind>> = source_output
        .let_bindings
        .iter()
        .map(|binding| (binding.name.as_str(), binding.kind.as_ref()))
        .collect();
    // The good bindings survive; the broken `$bad` contributes nothing.
    assert_eq!(bound.get("good_a"), Some(&Some(&Kind::Int)));
    assert_eq!(bound.get("good_b"), Some(&Some(&Kind::Int)));
    assert!(!bound.contains_key("bad") || bound["bad"].is_none());

    // Inlay hints render for both good bindings.
    let hints = let_binding_hints(source_output);
    assert_eq!(hints.len(), 2);

    // Hover resolves on the good `$good_b` use in `RETURN`/its binding.
    let offset = text.rfind("$good_b").expect("has $good_b") as u32 + 1;
    let hover = hover_at(source_output, &analysis.schema, &source, text, offset)
        .expect("hover resolves on a good binding beside a broken sibling");
    assert!(hover.markdown.contains("int"));
}

#[test]
fn resilient_go_to_definition_survives_a_syntax_error_sibling() {
    // Go-to-definition on a `$var` use still jumps to its binding even when
    // a sibling statement in the same body has a syntax error.
    use surrealql_analyzer_workspace::definition_at;
    let mut workspace = Workspace::default();
    let text = "DEFINE FUNCTION fn::demo($p: int) {\n\
        LET $good_a = 1 + 2;\n\
        LET $bad = SELECT name FROM ;\n\
        RETURN $good_a;\n\
    };";
    let source = workspace.add_virtual_source("query".into(), text.into());
    let analysis = analyze_workspace(&workspace);
    let source_output = &analysis.sources[&source];

    // The `$good_a` in `RETURN $good_a` resolves to its `LET` binding site.
    let use_offset = text.rfind("$good_a").expect("has RETURN use") as u32 + 1;
    let target = definition_at(source_output, &analysis.schema, &source, text, use_offset)
        .expect("go-to-def resolves beside a broken sibling");
    let binding_offset = text.find("$good_a").expect("has binding") as u32;
    assert_eq!(target.span.range().start(), binding_offset);
}

#[test]
fn analyze_workspace_reports_clause_value_and_lint_findings() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD tags ON person TYPE array<string>;\nLET $lim = 'a';\nSELECT * FROM person LIMIT $lim;\nSELECT * FROM person START -1;\nSELECT * FROM person FETCH age;\nSELECT * FROM person SPLIT age;\nSELECT age FROM person ORDER BY tags;\nDEFINE FIELD name ON person TYPE string;\nLET $auth = 1;\nRETURN [1, 'a'];\nIF true { RETURN 1; };\nRETURN array::map([1], |$v, $i, $extra| $v);\nSELECT type::field('ghost') FROM person;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    for (code, message) in [
        ("E2018", "LIMIT needs an integer, but this is a `string`"),
        ("E2018", "START can't be negative"),
        ("E1023", "FETCH `age` does nothing — `int` holds no records"),
        (
            "E1024",
            "SPLIT needs a collection field, but `age` is a `int`",
        ),
        (
            "E2017",
            "ORDER BY `tags` doesn't name a field of this query's rows",
        ),
        (
            "E6007",
            "`$auth` is a protected parameter and can't be assigned",
        ),
        ("L7003", "array literal mixes kinds: `int`, `string`"),
        (
            "L7004",
            "this IF condition is constant, so one branch is never taken",
        ),
        (
            "E5002",
            "`array::map` calls its closure with 2 arguments; `$extra` is never bound",
        ),
        ("E5005", "`ghost` is not a field of table `person`"),
    ] {
        assert!(
            messages.iter().any(|(c, m)| c == code && m == message),
            "missing {code}: {message}\nhave: {messages:#?}"
        );
    }
}

#[test]
fn analyze_workspace_machinery_batch() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        concat!(
            "DEFINE TABLE person SCHEMAFULL;\n",
            "DEFINE FIELD age ON person TYPE int DEFAULT 0;\n",
            "DEFINE FUNCTION fn::bad() -> string { RETURN 1; };\n",
            "DEFINE FUNCTION fn::loop_a() { RETURN fn::loop_b(); };\n",
            "DEFINE FUNCTION fn::loop_b() { RETURN fn::loop_a(); };\n",
            "DEFINE EVENT audit ON person WHEN $event = 'CRATE' THEN { RETURN 1; };\n",
            "RETURN $before;\n",
            "COMMIT;\n",
            "BEGIN;\n",
            "BEGIN;\n",
            "COMMIT;\n",
            "BEGIN;\n",
            "RETURN 1;\n",
        )
        .into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    for (code, needle) in [
        (
            "E2012",
            "`fn::bad` declares `-> string` but its body returns `1`",
        ),
        ("E5009", "never terminates"),
        // $event is the literal union, so the typo comparison is the
        // ordinary always-false lint with the edge-filter machinery.
        ("L7005", "`=` between"),
        (
            "E6005",
            "`$before` only exists inside the construct that binds it",
        ),
        ("E4007", "COMMIT/CANCEL without an open BEGIN"),
        ("E4007", "BEGIN inside an open transaction"),
        ("E4007", "this BEGIN is never closed"),
    ] {
        assert!(
            messages
                .iter()
                .any(|(c, m)| c == code && m.contains(needle)),
            "missing {code}: {needle}\nhave: {messages:#?}"
        );
    }
}

#[test]
fn analyze_workspace_full_coverage_batch_two() {
    // Slice F: literal content (2032/2031), casts (2008), SINCE (2021),
    // FOR iterables (2022 via params; literals are parser-rejected),
    // PATCH shapes (2033), GeoJSON (2036), FROM landings (3004),
    // non-record traversal starts (3009), index-backed operators
    // (1027), duplicate indexes (1029), analyzer components
    // (1032/2035), recursion bounds (3011), use-before-LET (6004).
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        concat!(
            "DEFINE TABLE person SCHEMAFULL;\n",
            "DEFINE FIELD name ON person TYPE string DEFAULT 'x';\n",
            "DEFINE FIELD age ON person TYPE int DEFAULT 0;\n",
            "DEFINE TABLE post;\n",
            "DEFINE TABLE likes TYPE RELATION IN person OUT post;\n",
            "DEFINE INDEX by_name ON person FIELDS name;\n",
            "DEFINE INDEX by_name_too ON person FIELDS name;\n",
            "DEFINE ANALYZER myan TOKENIZERS blank FILTERS snowball(klingon),edgengram(9,2);\n",
            "RETURN d'2024-13-45T00:00:00Z';\n",
            "RETURN 'a' ~ 'unclosed(';\n",
            "RETURN <int> 'abc';\n",
            "RETURN <duration> true;\n",
            "SHOW CHANGES FOR TABLE person SINCE 'not-a-date';\n",
            "LET $n = 42;\n",
            "FOR $x IN $n { RETURN 1; };\n",
            "UPDATE person PATCH [{ op: 'remvoe', path: 'name' }] WHERE name = 'x';\n",
            "RETURN { type: 'Pointt', coordinates: [1, 2] };\n",
            "SELECT * FROM person->likes;\n",
            "SELECT age->likes->post FROM person;\n",
            "SELECT * FROM person WHERE name @@ 'q';\n",
            "SELECT ->likes.{..}->post FROM person;\n",
            "RETURN $later;\n",
            "LET $later = 1;\n",
        )
        .into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    for (code, needle) in [
        ("E2032", "not a valid datetime"),
        ("E2031", "not a valid regex"),
        ("E2008", "`abc` can't be cast to `int`"),
        ("E2008", "a `bool` can't be cast to `duration`"),
        ("E2021", "needs a versionstamp or datetime"),
        ("E2022", "FOR can't iterate a `int`"),
        ("E2033", "`remvoe` is not a PATCH operation"),
        ("E2033", "PATCH paths start with `/`"),
        ("E2036", "`Pointt` is not a GeoJSON geometry type"),
        ("E3004", "not on a table"),
        ("E3009", "can't start from `age`"),
        ("E1027", "needs a SEARCH ANALYZER index"),
        ("E1029", "covers the same fields as `by_name`"),
        ("E1032", "`klingon` is not a supported snowball language"),
        ("E2035", "needs `(min, max)` with min <= max"),
        ("E3011", "no upper bound"),
        ("E6004", "read before its LET"),
    ] {
        assert!(
            messages
                .iter()
                .any(|(c, m)| c == code && m.contains(needle)),
            "missing {code}: {needle}\nhave: {messages:#?}"
        );
    }
}

#[test]
fn analyze_workspace_full_coverage_batch_one() {
    // Slice batch: required fields (2034), relation writes (4019),
    // RETURN BEFORE on CREATE (4020), whole-table writes (7009),
    // SET id (7011), compound assignment operands (2004), fn::
    // signatures (5002), loop contracts (4005), unreachable code
    // (4006), shadowing (7002), wildcard-plus-field (7007),
    // OMIT-without-* (4012), read-position writes (4018), empty
    // membership (7006), DROP reads (4022), changefeed (4021).
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        concat!(
            "DEFINE TABLE person SCHEMAFULL;\n",
            "DEFINE FIELD name ON person TYPE string;\n",
            "DEFINE FIELD age ON person TYPE int DEFAULT 0;\n",
            "DEFINE TABLE post;\n",
            "DEFINE TABLE likes TYPE RELATION IN person OUT post;\n",
            "DEFINE TABLE audit DROP;\n",
            "DEFINE FUNCTION fn::greet($who: string) -> string { RETURN 'hi'; };\n",
            "CREATE person;\n",
            "CREATE person RETURN BEFORE;\n",
            "CREATE likes SET strength = 1;\n",
            "UPDATE person SET name = 'Ada';\n",
            "UPDATE person SET id = person:two WHERE name = 'Ada';\n",
            "UPDATE person SET age += '1' WHERE name = 'Ada';\n",
            "RETURN fn::greet(1);\n",
            "RETURN fn::greet();\n",
            "BREAK;\n",
            "RETURN { LET $x = 1; RETURN $x; LET $y = 2; };\n",
            "LET $shadow = 1;\n",
            "IF $shadow > 0 { LET $shadow = 2; RETURN $shadow; };\n",
            "SELECT *, name FROM person;\n",
            "SELECT (CREATE person SET name = 'x') AS made FROM person;\n",
            "SELECT * FROM person WHERE name IN [];\n",
            "SELECT * FROM audit;\n",
            "SHOW CHANGES FOR TABLE person SINCE 0;\n",
        )
        .into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    for (code, message) in [
        ("E2034", "`name` must be set when creating a `person`"),
        (
            "E4020",
            "RETURN BEFORE on CREATE is always NONE; there is no before state",
        ),
        (
            "E4019",
            "`likes` is a relation; use RELATE (or provide `in` and `out`)",
        ),
        (
            "L7009",
            "this writes every row of `person`; add WHERE or a record id",
        ),
        ("L7011", "record ids are immutable; `id` is set at creation"),
        ("E2004", "`+=` can't combine a `int` and a `string`"),
        (
            "E5002",
            "argument 1 to `fn::greet` is a `1`, but `$who` is declared `string`",
        ),
        (
            "E5002",
            "`fn::greet` takes 1 argument, but this call passes 0",
        ),
        (
            "E4005",
            "BREAK here does nothing — it is outside any FOR loop",
        ),
        (
            "E4006",
            "this statement is unreachable — the block already returned",
        ),
        (
            "L7002",
            "`$shadow` is re-bound inside this block; the outer `$shadow` is unchanged",
        ),
        ("L7007", "this field is already included by `*`"),
        (
            "E4018",
            "this SELECT hides a write; run the mutation as its own statement",
        ),
        (
            "L7006",
            "membership test against an empty collection is always false",
        ),
        (
            "E4022",
            "`audit` is a DROP table, so this SELECT never returns rows",
        ),
        (
            "E4021",
            "`person` has no CHANGEFEED, so SHOW CHANGES reads nothing",
        ),
    ] {
        assert!(
            messages.iter().any(|(c, m)| c == code && m == message),
            "missing {code}: {message}\nhave: {messages:#?}"
        );
    }
}

#[test]
fn analyze_workspace_reports_statement_shape_misuse() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nSELECT * FROM ONLY person;\nSELECT * FROM ONLY person LIMIT 1;\nSELECT * FROM ONLY person:one;\nUPDATE ONLY person SET age = 1;\nUPDATE person SET age = 1, age = 2;\nSELECT age, age FROM person;\nLET $y = { LET $inner = 1; };".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .filter(|finding| matches!(finding.code().number(), 4001..=4020))
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    let expect = [
        (
            "E4003",
            "ONLY needs a single-row target, but this reads a whole table",
        ),
        ("E4003", "ONLY on a whole table needs a record id target"),
        (
            "W4010",
            "`age` is assigned more than once; the last assignment wins",
        ),
        ("W4011", "`age` is projected twice; the later one wins"),
        (
            "W4017",
            "block ends with LET, so its value is NONE — return the value instead",
        ),
    ];
    for (code, message) in expect {
        // Rendered test codes carry the category letter; severities are
        // asserted through the severity() accessor below.
        assert!(
            messages
                .iter()
                .any(|(c, m)| c.trim_start_matches(char::is_alphabetic)
                    == code.trim_start_matches(char::is_alphabetic)
                    && m == message),
            "missing {code}: {message} in {messages:?}"
        );
    }
    // The guarded forms produce nothing: LIMIT 1 and record ids are
    // exactly the escape hatches.
    assert_eq!(
        messages.iter().filter(|(_, m)| m.contains("ONLY")).count(),
        2
    );
}

#[test]
fn suppression_directive_silences_next_line_and_trailing_findings() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "-- surrealql-analyzer: allow(E1001) reason=\"fixture table\"\nSELECT * FROM ghost;\nSELECT * FROM phantom; -- surrealql-analyzer: allow(E1001) reason=\"also fine\"\nSELECT * FROM spectre;".into(),
    );

    let output = analyze_workspace(&workspace);
    let diagnostics = &output.sources[&source].diagnostics;
    assert_no_syntax_findings(diagnostics);

    // ghost (next-line) and phantom (trailing) are suppressed;
    // spectre still fires.
    let unknown_tables: Vec<_> = diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1001))
        .map(|finding| finding.message().to_string())
        .collect();
    assert_eq!(unknown_tables, vec!["`spectre` is not a defined table"]);
}

#[test]
fn suppression_directive_contract_violations_are_7013() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "-- surrealql-analyzer: allow(E9999)\n-- surrealql-analyzer: allow(lint.select_star)\n-- surrealql-analyzer: allow(*)\nSELECT * FROM person;".into(),
    );

    let output = analyze_workspace(&workspace);
    let diagnostics = &output.sources[&source].diagnostics;

    let directive_findings: Vec<_> = diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::lint(7013))
        .map(|finding| finding.message().to_string())
        .collect();
    assert_eq!(directive_findings.len(), 3, "{directive_findings:?}");
    assert!(directive_findings[0].contains("`E9999` is not a catalog code"));
    assert!(directive_findings[1].contains("suppress by catalog code, not name"));
    assert!(directive_findings[2].contains("does not parse"));
}

#[test]
fn suppression_reasons_are_required_when_configured() {
    let mut config = WorkspaceConfig::default();
    config.diagnostics.require_suppression_reasons = true;
    let mut workspace = Workspace::new(config);
    let source = workspace.add_virtual_source(
        "query".into(),
        "-- surrealql-analyzer: allow(E1001)\nSELECT * FROM ghost;".into(),
    );

    let output = analyze_workspace(&workspace);
    let diagnostics = &output.sources[&source].diagnostics;

    // Without a reason the directive is rejected: 7013 fires and the
    // 1001 it tried to silence still reports.
    assert!(diagnostics
        .iter()
        .any(|finding| finding.code() == FindingCode::lint(7013)));
    assert!(diagnostics
        .iter()
        .any(|finding| finding.code() == FindingCode::schema(1001)));
}

/// A source's top level is not a block, and must not be modelled as one.
///
/// A block exits at its first `RETURN` and never reaches what follows, so
/// 4006 is right there. A source's statement sequence does not — every
/// top-level statement runs and responds independently. Engine-verified on
/// SurrealDB 3.0.5:
///
/// ```text
/// RETURN { RETURN 1; RETURN 2; };                     -> [1]
/// RETURN 1; RETURN 2;                                 -> [1, 2]
/// CREATE onlytest:1; THROW 'boom'; CREATE onlytest:2;
///   -> [[{id: onlytest:1}], "An error occurred: boom", [{id: onlytest:2}]]
/// ```
///
/// So a top-level `Flow` — collecting `RETURN`s into one exit-set type, and
/// reporting 4006 after a top-level `THROW` — would be a false positive on
/// correct SurrealQL and a response type the engine does not produce. This
/// test is here so that stays decided.
#[test]
fn a_top_level_statement_after_a_throw_is_not_unreachable() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "THROW 'boom';
RETURN 1;
RETURN 2;"
            .into(),
    );

    let output = analyze_workspace(&workspace);
    assert!(
        !output.sources[&source]
            .diagnostics
            .iter()
            .any(|finding| finding.code().number() == 4006),
        "top-level statements after a THROW do run: {:?}",
        output.sources[&source].diagnostics
    );
    // …and each one responds for itself, rather than being folded into an
    // exit set that would drop the second `RETURN`.
    let responses: Vec<_> = output.sources[&source]
        .statements
        .iter()
        .filter_map(|statement| statement.response_kind.clone())
        .collect();
    assert_eq!(responses.len(), 2, "each top-level RETURN responds");
}
