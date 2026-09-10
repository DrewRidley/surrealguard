//! The DDL contracts, quantified over every modeled schema-object kind.
//!
//! Three contracts, each stated once in the catalog and owed by every kind of
//! `DEFINE`/`REMOVE`/`ALTER` alike:
//!
//! * 1022 — a definition does not silently redefine. `OVERWRITE` states the
//!   intent to replace, `IF NOT EXISTS` the intent to keep the first; neither
//!   is a duplicate. A `REMOVE` in between makes the second definition the
//!   only one.
//! * 1021 — a removal names something that exists, and the removal takes
//!   effect: the statements after it no longer see the object.
//! * 5010 — an event does not trigger itself, directly or through the events
//!   of the tables it writes. A `WHEN` that narrows `$event` is honoured, so
//!   the canonical `WHEN $event = 'CREATE' THEN UPDATE $after.id` is not a
//!   loop.
//!
//! Plus `ALTER TABLE`'s effect on the catalog, which the field and read
//! checks downstream consume.
//!
//! Every positive case has its near-miss beside it, because a check that
//! fires on the intended form is worse than one that misses the accident.

use surrealguard_diagnostics::Finding;
use surrealguard_workspace::{analyze_workspace, Workspace, WorkspaceAnalysis};

fn analyze(schema: &str) -> WorkspaceAnalysis {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source("schema".into(), schema.into());
    analyze_workspace(&workspace)
}

fn analyze_two(first: &str, second: &str) -> WorkspaceAnalysis {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source("a".into(), first.into());
    workspace.add_virtual_source("b".into(), second.into());
    analyze_workspace(&workspace)
}

fn with_code(output: &WorkspaceAnalysis, code: u16) -> Vec<&Finding> {
    output
        .diagnostics
        .iter()
        .filter(|finding| finding.code().number() == code)
        .collect()
}

fn messages(output: &WorkspaceAnalysis, code: u16) -> Vec<String> {
    with_code(output, code)
        .into_iter()
        .map(|finding| finding.message().to_string())
        .collect()
}

fn assert_none(output: &WorkspaceAnalysis, code: u16, context: &str) {
    let found = messages(output, code);
    assert!(
        found.is_empty(),
        "{context}: expected no {code}, got {found:?}"
    );
}

// ---- 1022: a definition does not silently redefine ----

/// One template per kind: the schema that defines the object once, and the
/// head of a second definition of it with `{flag}` in the flag position.
const DUPLICATE_SITES: &[(&str, &str, &str)] = &[
    (
        "index",
        "DEFINE TABLE t; DEFINE FIELD a ON t TYPE int;\nDEFINE INDEX idx ON t FIELDS a;\n",
        "DEFINE INDEX {flag} idx ON t FIELDS a;",
    ),
    (
        "event",
        "DEFINE TABLE t;\nDEFINE EVENT ev ON t THEN { RETURN 1; };\n",
        "DEFINE EVENT {flag} ev ON t THEN { RETURN 1; };",
    ),
    (
        "function",
        "DEFINE FUNCTION fn::f() { RETURN 1; };\n",
        "DEFINE FUNCTION {flag} fn::f() { RETURN 2; };",
    ),
    (
        "param",
        "DEFINE PARAM $p VALUE 1;\n",
        "DEFINE PARAM {flag} $p VALUE 2;",
    ),
    (
        "analyzer",
        "DEFINE ANALYZER a TOKENIZERS blank;\n",
        "DEFINE ANALYZER {flag} a TOKENIZERS blank;",
    ),
];

#[test]
fn a_second_definition_without_a_flag_is_a_duplicate() {
    for (kind, first, second) in DUPLICATE_SITES {
        let schema = format!("{first}{}", second.replace("{flag} ", ""));
        let output = analyze(&schema);
        let found = with_code(&output, 1022);
        assert_eq!(
            found.len(),
            1,
            "{kind}: expected exactly one 1022 for\n{schema}\ngot {:?}",
            output.diagnostics
        );
        let finding = found[0];
        assert!(
            finding
                .message()
                .ends_with("is already defined; this DEFINE silently replaces the earlier one"),
            "{kind}: {}",
            finding.message()
        );
        // The finding sits on the SECOND definition and points back at the first.
        let first_len = first.len() as u32;
        assert!(
            finding.span().range().start() >= first_len,
            "{kind}: 1022 should span the redefinition, got {:?}",
            finding.span().range()
        );
        assert!(
            finding
                .related()
                .iter()
                .any(|note| note.span.range().start() < first_len
                    && note.message.ends_with("is defined here")),
            "{kind}: expected a note at the first definition, got {:?}",
            finding.related()
        );
        assert!(
            finding
                .help()
                .iter()
                .any(|help| help.message.contains("OVERWRITE")),
            "{kind}: help should name OVERWRITE, got {:?}",
            finding.help()
        );
    }
}

#[test]
fn overwrite_and_if_not_exists_are_not_duplicates() {
    for (kind, first, second) in DUPLICATE_SITES {
        for flag in ["OVERWRITE", "IF NOT EXISTS"] {
            let schema = format!("{first}{}", second.replace("{flag}", flag));
            let output = analyze(&schema);
            assert_none(&output, 1022, &format!("{kind} with {flag}:\n{schema}"));
        }
    }
}

#[test]
fn a_single_definition_is_never_its_own_duplicate() {
    // Functions are hoisted into the catalog before the walk, so the
    // definition sees itself there — that must not read as a duplicate.
    for (kind, first, _) in DUPLICATE_SITES {
        let output = analyze(first);
        assert_none(&output, 1022, &format!("{kind} defined once"));
    }
}

#[test]
fn a_duplicate_across_sources_reports_too() {
    // Tables, indexes, events, params and analyzers: the other file's
    // definition is in the catalog when this file's is walked.
    let output = analyze_two(
        "DEFINE TABLE t;\nDEFINE PARAM $p VALUE 1;\nDEFINE ANALYZER a TOKENIZERS blank;",
        "DEFINE TABLE t;\nDEFINE PARAM $p VALUE 2;\nDEFINE ANALYZER a TOKENIZERS blank;",
    );
    assert_eq!(
        with_code(&output, 1022).len(),
        6,
        "each file sees the other's definition: {:?}",
        output.diagnostics
    );
    // Functions are the known gap: the within-source hoist replaces the other
    // file's `fn::f` with this file's own before the walk, so neither side sees
    // a foreign definition. Not asserted either way here — see
    // `check_duplicate_function`'s doc.
}

// ---- 1021: REMOVE removes something that exists ----

/// Per kind: what the object hangs off (defined once, up front), the
/// definition, the removal of it, and a removal of something never defined.
const REMOVE_SITES: &[(&str, &str, &str, &str, &str)] = &[
    (
        "event",
        "DEFINE TABLE t;\n",
        "DEFINE EVENT ev ON t THEN { RETURN 1; };\n",
        "REMOVE EVENT ev ON t;",
        "REMOVE EVENT ghost ON t;",
    ),
    (
        "function",
        "",
        "DEFINE FUNCTION fn::f() { RETURN 1; };\n",
        "REMOVE FUNCTION fn::f;",
        "REMOVE FUNCTION fn::ghost;",
    ),
    (
        "param",
        "",
        "DEFINE PARAM $p VALUE 1;\n",
        "REMOVE PARAM $p;",
        "REMOVE PARAM $ghost;",
    ),
    (
        "analyzer",
        "",
        "DEFINE ANALYZER a TOKENIZERS blank;\n",
        "REMOVE ANALYZER a;",
        "REMOVE ANALYZER ghost;",
    ),
];

#[test]
fn removing_something_undefined_reports_and_removing_a_definition_does_not() {
    for (kind, prelude, define, remove, remove_ghost) in REMOVE_SITES {
        let output = analyze(&format!("{prelude}{define}{remove_ghost}"));
        let found = messages(&output, 1021);
        assert_eq!(
            found.len(),
            1,
            "{kind}: {remove_ghost} should report 1021 once, got {:?}",
            output.diagnostics
        );

        let output = analyze(&format!("{prelude}{define}{remove}"));
        assert_none(&output, 1021, &format!("{kind}: {remove} after its DEFINE"));
    }
}

#[test]
fn removing_an_event_on_an_unknown_table_reports() {
    let output = analyze("REMOVE EVENT ev ON ghost;");
    assert_eq!(
        messages(&output, 1021),
        vec!["REMOVE EVENT `ev` targets `ghost`, which is not a defined table"]
    );
}

#[test]
fn a_removal_takes_effect_in_the_catalog() {
    let output = analyze(
        "DEFINE TABLE t; DEFINE FIELD a ON t TYPE int;\n\
         DEFINE INDEX idx ON t FIELDS a;\n\
         DEFINE EVENT ev ON t THEN { RETURN 1; };\n\
         DEFINE FUNCTION fn::f() { RETURN 1; };\n\
         DEFINE PARAM $p VALUE 1;\n\
         DEFINE ANALYZER a TOKENIZERS blank;\n\
         REMOVE INDEX idx ON t;\n\
         REMOVE EVENT ev ON t;\n\
         REMOVE FUNCTION fn::f;\n\
         REMOVE PARAM $p;\n\
         REMOVE ANALYZER a;",
    );
    let table = &output.schema.tables["t"];
    assert!(table.indexes.is_empty(), "{:?}", table.indexes);
    assert!(table.events.is_empty(), "{:?}", table.events);
    assert!(output.schema.functions.is_empty());
    assert!(output.schema.params.is_empty());
    assert!(output.schema.analyzers.is_empty());
    assert_none(&output, 1021, "every REMOVE names a definition");
}

#[test]
fn a_removal_is_seen_by_the_statements_after_it() {
    // A second REMOVE of the same object finds nothing.
    let output = analyze(
        "DEFINE TABLE t; DEFINE FIELD a ON t TYPE int;\n\
         DEFINE INDEX idx ON t FIELDS a;\n\
         REMOVE INDEX idx ON t;\n\
         REMOVE INDEX idx ON t;",
    );
    assert_eq!(
        messages(&output, 1012),
        vec!["`t` has no index `idx` (REMOVE)"],
        "{:?}",
        output.diagnostics
    );

    // And a call to a removed function no longer resolves.
    let output = analyze(
        "DEFINE FUNCTION fn::f() { RETURN 1; };\n\
         REMOVE FUNCTION fn::f;\n\
         RETURN fn::f();",
    );
    assert_eq!(
        with_code(&output, 5001).len(),
        1,
        "the call after REMOVE FUNCTION is unresolved: {:?}",
        output.diagnostics
    );
}

#[test]
fn redefining_after_a_removal_is_not_a_duplicate() {
    for (kind, prelude, define, remove, _) in REMOVE_SITES {
        let schema = format!("{prelude}{define}{remove}\n{define}");
        let output = analyze(&schema);
        assert_none(
            &output,
            1022,
            &format!("{kind}: DEFINE, REMOVE, DEFINE\n{schema}"),
        );
        assert_none(
            &output,
            1021,
            &format!("{kind}: DEFINE, REMOVE, DEFINE\n{schema}"),
        );
    }
    let output = analyze(
        "DEFINE TABLE t; DEFINE FIELD a ON t TYPE int;\n\
         DEFINE INDEX idx ON t FIELDS a;\n\
         REMOVE INDEX idx ON t;\n\
         DEFINE INDEX idx ON t FIELDS a;",
    );
    assert_none(&output, 1022, "index: DEFINE, REMOVE, DEFINE");
    assert_none(&output, 1029, "index: DEFINE, REMOVE, DEFINE");
}

// ---- ALTER TABLE: the effect lands in the catalog ----

#[test]
fn alter_table_rewrites_the_schema_mode_and_drop_flags() {
    let output = analyze("DEFINE TABLE t SCHEMALESS;\nALTER TABLE t SCHEMAFULL;");
    assert!(output.schema.tables["t"].schemafull);
    assert!(!output.schema.tables["t"].drop_table);

    let output = analyze("DEFINE TABLE t SCHEMAFULL;\nALTER TABLE t SCHEMALESS DROP;");
    assert!(!output.schema.tables["t"].schemafull);
    assert!(output.schema.tables["t"].drop_table);

    // A permissions-only ALTER leaves both flags alone.
    let output = analyze("DEFINE TABLE t SCHEMAFULL;\nALTER TABLE t PERMISSIONS NONE;");
    assert!(output.schema.tables["t"].schemafull);
    assert!(!output.schema.tables["t"].drop_table);
}

#[test]
fn alter_table_drop_is_seen_by_the_reads_after_it() {
    let output = analyze(
        "DEFINE TABLE t SCHEMAFULL;\n\
         SELECT * FROM t;\n\
         ALTER TABLE t DROP;\n\
         SELECT * FROM t;",
    );
    let found = with_code(&output, 4022);
    assert_eq!(found.len(), 1, "{:?}", output.diagnostics);
    // The read BEFORE the ALTER is fine; only the one after it is flagged.
    let alter_at = output
        .sources
        .values()
        .next()
        .and_then(|source| {
            source
                .statements
                .iter()
                .find(|statement| statement.kind == "alter")
        })
        .map(|statement| statement.span.range().start())
        .expect("the ALTER statement");
    assert!(found[0].span().range().start() > alter_at);
}

#[test]
fn alter_table_schemafull_then_an_unknown_field_write_reports() {
    let output = analyze(
        "DEFINE TABLE t SCHEMALESS;\n\
         DEFINE FIELD a ON t TYPE int;\n\
         ALTER TABLE t SCHEMAFULL;\n\
         CREATE t SET b = 1;",
    );
    assert_eq!(
        with_code(&output, 1002).len(),
        1,
        "`b` is not a field of the now-SCHEMAFULL `t`: {:?}",
        output.diagnostics
    );
}

#[test]
fn alter_of_an_unknown_table_reports_and_changes_nothing() {
    let output = analyze("ALTER TABLE ghost SCHEMAFULL;");
    assert_eq!(
        with_code(&output, 1001).len(),
        1,
        "{:?}",
        output.diagnostics
    );
    assert!(output.schema.tables.is_empty());
}

// ---- 5010: events do not trigger themselves ----

#[test]
fn an_event_that_writes_its_own_table_can_trigger_itself() {
    let output = analyze(
        "DEFINE TABLE t;\n\
         DEFINE EVENT ev ON t THEN { UPDATE t SET touched = true; };",
    );
    let found = with_code(&output, 5010);
    assert_eq!(found.len(), 1, "{:?}", output.diagnostics);
    assert_eq!(
        found[0].message(),
        "`ev` can trigger itself: its body writes `t` (UPDATE), which fires `ev` again"
    );
    // Spanned at the event name, with the write attached.
    assert_eq!(
        found[0].span().range().start() as usize,
        "DEFINE TABLE t;\nDEFINE EVENT ".len()
    );
    assert!(
        found[0]
            .related()
            .iter()
            .any(|note| note.message == "`t` is written here (UPDATE)"),
        "{:?}",
        found[0].related()
    );
}

#[test]
fn a_when_that_narrows_the_event_kind_breaks_the_loop() {
    // The canonical pattern: fire on CREATE, then UPDATE the new row. The
    // UPDATE does not re-fire a CREATE-only event.
    for when in [
        "$event = 'CREATE'",
        "$event == 'CREATE'",
        "$event IN ['CREATE', 'DELETE']",
        "$event != 'UPDATE'",
        "$event = 'CREATE' AND $after.name != NONE",
        "NOT ($event = 'UPDATE')",
    ] {
        let schema = format!(
            "DEFINE TABLE t;\n\
             DEFINE EVENT ev ON t WHEN {when} THEN {{ UPDATE $after.id SET touched = true; }};"
        );
        let output = analyze(&schema);
        assert_none(&output, 5010, &format!("WHEN {when}"));
    }
}

#[test]
fn a_when_that_admits_the_write_kind_is_still_a_loop() {
    for (when, write) in [
        ("$event = 'UPDATE'", "UPDATE $after.id SET n = 1"),
        ("$event IN ['CREATE', 'UPDATE']", "UPDATE $value SET n = 1"),
        ("$event = 'DELETE'", "DELETE $before"),
        ("$event = 'CREATE'", "CREATE t SET n = 1"),
        ("$event = 'CREATE'", "INSERT INTO t { n: 1 }"),
        ("$event = 'UPDATE'", "UPSERT t:one SET n = 1"),
        // A guard that says nothing about `$event` admits every kind.
        ("$after.n > 3", "UPDATE t SET n = 1"),
    ] {
        let schema = format!(
            "DEFINE TABLE t;\n\
             DEFINE EVENT ev ON t WHEN {when} THEN {{ {write}; }};"
        );
        let output = analyze(&schema);
        assert_eq!(
            with_code(&output, 5010).len(),
            1,
            "WHEN {when} THEN {write}: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn writing_a_table_without_events_is_not_a_loop() {
    let output = analyze(
        "DEFINE TABLE t;\n\
         DEFINE TABLE log;\n\
         DEFINE EVENT ev ON t THEN { CREATE log SET at = time::now(); };",
    );
    assert_none(&output, 5010, "write to an event-less table");

    // A chain that ends at an event-less table is not a cycle either.
    let output = analyze(
        "DEFINE TABLE t;\n\
         DEFINE TABLE log;\n\
         DEFINE TABLE audit;\n\
         DEFINE EVENT ev ON t THEN { CREATE log SET at = time::now(); };\n\
         DEFINE EVENT logged ON log THEN { CREATE audit SET at = time::now(); };",
    );
    assert_none(&output, 5010, "t -> log -> audit, no cycle");
}

#[test]
fn a_cycle_through_two_tables_reports_on_every_event_in_it() {
    let output = analyze(
        "DEFINE TABLE a;\n\
         DEFINE TABLE b;\n\
         DEFINE EVENT ping ON a THEN { CREATE b SET n = 1; };\n\
         DEFINE EVENT pong ON b THEN { CREATE a SET n = 1; };",
    );
    let mut found = messages(&output, 5010);
    found.sort();
    assert_eq!(
        found,
        vec![
            "`ping` can trigger itself through other events: ping ON a -> pong ON b -> ping ON a",
            "`pong` can trigger itself through other events: pong ON b -> ping ON a -> pong ON b",
        ]
    );

    // Break the cycle at one edge and nothing reports.
    let output = analyze(
        "DEFINE TABLE a;\n\
         DEFINE TABLE b;\n\
         DEFINE EVENT ping ON a WHEN $event = 'UPDATE' THEN { CREATE b SET n = 1; };\n\
         DEFINE EVENT pong ON b THEN { CREATE a SET n = 1; };",
    );
    assert_none(
        &output,
        5010,
        "pong CREATEs a, but ping fires on UPDATE only",
    );
}

#[test]
fn a_cycle_across_sources_reports_in_each_defining_source() {
    let output = analyze_two(
        "DEFINE TABLE a;\nDEFINE TABLE b;\n\
         DEFINE EVENT ping ON a THEN { CREATE b SET n = 1; };",
        "DEFINE EVENT pong ON b THEN { CREATE a SET n = 1; };",
    );
    let found = with_code(&output, 5010);
    assert_eq!(found.len(), 2, "{:?}", output.diagnostics);
    let mut sources: Vec<String> = found
        .iter()
        .map(|finding| finding.span().source().to_string())
        .collect();
    sources.sort();
    assert_eq!(sources.len(), 2);
    assert_ne!(sources[0], sources[1], "one finding per defining source");
}

#[test]
fn a_write_nested_in_the_body_still_counts() {
    let output = analyze(
        "DEFINE TABLE t;\n\
         DEFINE EVENT ev ON t THEN {\n\
           IF $after.n > 3 {\n\
             LET $rows = (UPDATE t SET n = 0);\n\
           };\n\
         };",
    );
    assert_eq!(
        with_code(&output, 5010).len(),
        1,
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn a_removed_event_is_not_part_of_any_cycle() {
    let output = analyze(
        "DEFINE TABLE t;\n\
         DEFINE EVENT ev ON t THEN { UPDATE t SET n = 1; };\n\
         REMOVE EVENT ev ON t;",
    );
    assert_none(&output, 5010, "the event was removed");
}
