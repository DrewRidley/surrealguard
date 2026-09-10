//! Schema `DEFINE` checks and the schema index: tables, fields, indexes,
//! events, params, source-order application, table-reference validation,
//! and back-reference (`<~table`) resolution.

use surrealdb_types::{Kind, KindLiteral};
use surrealguard_diagnostics::FindingCode;
use surrealguard_workspace::schema::FieldPath;
use surrealguard_workspace::{analyze_workspace, render_kind, PartialReason, Workspace};

use crate::support::{assert_no_syntax_findings, codes};

#[test]
fn computed_back_reference_index_selects_the_element_record() {
    // `COMPUTED <~T[0]` selects ONE back-reference record, not the whole
    // `array<record<T>>` the bare `<~T` yields.
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE organization_billing SCHEMAFULL;\n\
         DEFINE FIELD org ON organization_billing TYPE record<organization> REFERENCE;\n\
         DEFINE TABLE organization SCHEMAFULL;\n\
         DEFINE FIELD billing ON organization COMPUTED <~organization_billing[0];"
            .into(),
    );
    let query =
        workspace.add_virtual_source("query".into(), "SELECT billing FROM organization;".into());

    let output = analyze_workspace(&workspace);
    let rendered = render_kind(
        output.sources[&query]
            .response_kind
            .as_ref()
            .expect("response kind"),
    );
    assert!(
        rendered.contains("billing: record<organization_billing>"),
        "expected the element record, got {rendered}"
    );
}

/// The 1001 findings raised for a one-source schema workspace.
fn unknown_table_findings(schema: &str) -> Vec<String> {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source("schema".into(), schema.into());
    analyze_workspace(&workspace)
        .diagnostics
        .iter()
        .filter(|finding| finding.code().number() == 1001)
        .map(|finding| finding.message().to_string())
        .collect()
}

#[test]
fn a_back_reference_to_an_undefined_table_is_an_unknown_table_error() {
    // `<~passkey` with no `passkey` table is a typo. It used to type as
    // `unknown` with zero diagnostics, silently hiding the mistake.
    let findings = unknown_table_findings(
        "DEFINE TABLE account SCHEMAFULL;\n\
         DEFINE FIELD passkeys ON account COMPUTED <~passkey;",
    );
    assert_eq!(
        findings,
        vec!["`passkey` is not a defined table".to_string()]
    );
}

#[test]
fn a_back_reference_to_a_table_without_a_backlink_stays_silent() {
    // `task` exists but its REFERENCE fields point elsewhere, so the
    // traversal is genuinely unresolvable — `any` is correct and there is
    // nothing to report. (The workshop oracle's real `<~task` shape.)
    let findings = unknown_table_findings(
        "DEFINE TABLE project SCHEMAFULL;\n\
         DEFINE TABLE task SCHEMAFULL;\n\
         DEFINE FIELD project ON task TYPE record<project> REFERENCE;\n\
         DEFINE TABLE sprint SCHEMAFULL;\n\
         DEFINE FIELD tasks ON sprint COMPUTED <~task;",
    );
    assert!(findings.is_empty(), "unexpected findings: {findings:?}");
}

/// The two halves of a mutual record reference. `PROJECT` traverses
/// `<~task`; `TASK` carries the `record<project> REFERENCE` field that is
/// the only thing making that traversal provable. Neither half can be
/// written "first" in any meaningful sense — the relationship is a cycle.
const BACKREF_PROJECT: &str = "DEFINE TABLE project SCHEMAFULL;\n\
                               DEFINE FIELD tasks ON project COMPUTED <~task;";

const BACKREF_TASK: &str = "DEFINE TABLE task SCHEMAFULL;\n\
                            DEFINE FIELD project ON task TYPE record<project> REFERENCE;";

/// Everything `project.tasks` resolves to, for a workspace whose two schema
/// sources are registered in `order`: the kind recorded in the run-wide
/// **schema index**, the kind a **query** projects, and every 1001 message.
/// All three are order-independent facts, so the tuple must not vary.
fn back_reference_facts(order: [&str; 2]) -> (String, String, Vec<String>) {
    let mut workspace = Workspace::default();
    for (n, text) in order.iter().enumerate() {
        workspace.add_virtual_source(format!("schema_{n}"), (*text).into());
    }
    let query = workspace.add_virtual_source("query".into(), "SELECT tasks FROM project;".into());
    let output = analyze_workspace(&workspace);

    let indexed = output
        .schema
        .table("project")
        .and_then(|table| table.fields.get("tasks"))
        .and_then(|field| field.kind.as_ref())
        .map_or_else(|| "unknown".to_string(), render_kind);
    let projected = output.sources[&query]
        .response_kind
        .as_ref()
        .map_or_else(|| "unknown".to_string(), render_kind);
    let findings = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code().number() == 1001)
        .map(|finding| finding.message().to_string())
        .collect();
    (indexed, projected, findings)
}

#[test]
fn a_back_reference_resolves_in_either_declaration_order() {
    // The invariant, not the value: the same schema written in the two
    // possible file orders must produce the same answer. It did not — the
    // resolution asked the incrementally-built catalog, which by
    // construction holds only what precedes the statement, so `<~task`
    // typed only when `task` happened to sort first.
    let target_last = back_reference_facts([BACKREF_PROJECT, BACKREF_TASK]);
    let target_first = back_reference_facts([BACKREF_TASK, BACKREF_PROJECT]);
    assert_eq!(
        target_last, target_first,
        "renaming the schema files changed the analysis"
    );

    let (indexed, projected, findings) = target_last;
    assert_eq!(indexed, "array<record<task>>");
    // …and the schema index agrees with the query path. The run-wide
    // catalog used to hold `unknown` for a field a SELECT typed correctly,
    // because a query source sees every OTHER source while the run-wide
    // catalog only saw what preceded it.
    assert_eq!(projected, format!("array<{{ tasks: {indexed} }}>"));
    assert!(findings.is_empty(), "unexpected findings: {findings:?}");
}

#[test]
fn a_back_reference_target_defined_later_in_the_same_source_is_not_unknown() {
    // The within-source order of the same pair. `<~task` above `DEFINE
    // TABLE task` raised a false 1001 whose own help text contradicted it
    // ("no `DEFINE TABLE task` exists in the workspace" — it does).
    let findings = unknown_table_findings(&format!("{BACKREF_PROJECT}\n{BACKREF_TASK}"));
    assert!(findings.is_empty(), "unexpected findings: {findings:?}");
}

#[test]
fn analyze_workspace_indexes_define_table_declarations() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE person;\nDEFINE TABLE company SCHEMAFULL;".into(),
    );

    let output = analyze_workspace(&workspace);

    assert_eq!(output.schema.tables.len(), 2);
    assert_eq!(output.schema.tables["person"].name, "person");
    assert_eq!(output.schema.tables["company"].source, source);
    let span = &output.schema.tables["person"].name_span;
    assert_eq!(span.source(), &source);
    assert_eq!(span.range().start(), 13);
    assert_eq!(span.range().end(), 19);
}

#[test]
fn analyze_workspace_reports_duplicate_table_declarations() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE person;\nDEFINE TABLE person;".into(),
    );

    let output = analyze_workspace(&workspace);

    assert_eq!(output.schema.tables.len(), 1);
    let duplicates: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1022))
        .collect();
    assert_eq!(duplicates.len(), 1);
    assert_eq!(
        duplicates[0].message(),
        "`person` is already defined; this DEFINE silently replaces the earlier one"
    );
    assert_eq!(duplicates[0].span().range().start(), 34);
    assert_eq!(duplicates[0].span().range().end(), 40);
}

// ---- 1022: IF NOT EXISTS is a deliberate no-op, as OVERWRITE is a
// deliberate replacement ----

#[test]
fn define_if_not_exists_is_not_a_duplicate_definition() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE IF NOT EXISTS t;\n\
         DEFINE TABLE IF NOT EXISTS t;\n\
         DEFINE FIELD IF NOT EXISTS a ON t TYPE int;\n\
         DEFINE FIELD IF NOT EXISTS a ON t TYPE int;"
            .into(),
    );
    let output = analyze_workspace(&workspace);
    assert_eq!(codes(&output, 1022), 0, "{:?}", output.diagnostics);

    // A plain redefinition is still the duplicate it always was.
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE t;\nDEFINE TABLE t;\n\
         DEFINE FIELD a ON t TYPE int;\nDEFINE FIELD a ON t TYPE int;"
            .into(),
    );
    let output = analyze_workspace(&workspace);
    assert_eq!(codes(&output, 1022), 2, "{:?}", output.diagnostics);
}

#[test]
fn field_element_type_definition_is_not_a_duplicate_of_the_base_field() {
    let mut workspace = Workspace::default();
    // `arr[*]` types the ELEMENTS of the `arr` array — distinct from the
    // base field, though both collapse to the same dotted path. It must
    // not be flagged as a duplicate definition (1022). A genuine plain
    // redefinition still is.
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE t SCHEMAFULL;\n\
         DEFINE FIELD arr ON t TYPE array;\n\
         DEFINE FIELD arr[*] ON t TYPE object;\n\
         DEFINE FIELD arr[*].price ON t TYPE string;\n\
         DEFINE FIELD dup ON t TYPE int;\n\
         DEFINE FIELD dup ON t TYPE int;"
            .into(),
    );

    let output = analyze_workspace(&workspace);

    let duplicates: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1022))
        .collect();
    // Only the genuine plain `dup` redefinition is a duplicate.
    assert_eq!(duplicates.len(), 1, "unexpected duplicates: {duplicates:?}");
    assert!(duplicates[0].message().contains("dup"));
}

#[test]
fn a_valid_write_to_a_field_with_descendant_definitions_is_not_a_type_error() {
    let mut workspace = Workspace::default();
    // Two of the most common real schema shapes. A descendant definition
    // used to REPLACE the parent's declared kind, so `items` read as a bare
    // object and `cfg` lost its optionality — both of these writes were
    // error-severity 2001s on valid SurrealQL, which aborts `generate` for
    // the whole project.
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE t SCHEMAFULL;\n\
         DEFINE FIELD items ON t TYPE array<object>;\n\
         DEFINE FIELD items[*] ON t TYPE object;\n\
         DEFINE FIELD items[*].price ON t TYPE string;\n\
         DEFINE FIELD cfg ON t TYPE option<object>;\n\
         DEFINE FIELD cfg.theme ON t TYPE string;\n\
         CREATE t SET items = [{ price: '1' }], cfg = NONE;\n\
         CREATE t SET items = [], cfg = { theme: 'dark' };"
            .into(),
    );

    let output = analyze_workspace(&workspace);

    assert_eq!(codes(&output, 2001), 0, "{:?}", output.diagnostics);
}

#[test]
fn a_wrong_write_to_a_refined_field_is_still_a_type_error() {
    let mut workspace = Workspace::default();
    // Refining must not turn into blanket permissiveness: the element type
    // the descendants declare is enforced, and the parent's own shape is
    // still a contract.
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE t SCHEMAFULL;\n\
         DEFINE FIELD items ON t TYPE array<object>;\n\
         DEFINE FIELD items[*].price ON t TYPE string;\n\
         CREATE t SET items = [{ price: 1 }];\n\
         CREATE t SET items = 'nope';"
            .into(),
    );

    let output = analyze_workspace(&workspace);

    assert_eq!(codes(&output, 2001), 2, "{:?}", output.diagnostics);
}

#[test]
fn a_subfield_under_a_scalar_parent_fires_1025_and_leaves_the_parent_writable() {
    let mut workspace = Workspace::default();
    // `title` is a string, so `title.sub` describes a member that can never
    // exist. Report the definition (1025) — and keep `title` a string, so
    // the perfectly valid `SET title = 'hello'` stays valid.
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE t SCHEMAFULL;\n\
         DEFINE FIELD title ON t TYPE string;\n\
         DEFINE FIELD title.sub ON t TYPE string;\n\
         CREATE t SET title = 'hello';"
            .into(),
    );

    let output = analyze_workspace(&workspace);

    assert_eq!(codes(&output, 1025), 1, "{:?}", output.diagnostics);
    assert_eq!(codes(&output, 2001), 0, "{:?}", output.diagnostics);
}

#[test]
fn a_subfield_under_an_object_shaped_or_undeclared_parent_never_fires_1025() {
    let mut workspace = Workspace::default();
    // Every shape with room for a subfield stays silent: an undeclared
    // parent, a bare/FLEXIBLE `object`, an `option<object>`, a literal
    // object, an untyped parent, and a collection reached through `[*]`.
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE t SCHEMAFULL;\n\
         DEFINE FIELD undeclared.name ON t TYPE string;\n\
         DEFINE FIELD open ON t TYPE object;\n\
         DEFINE FIELD open.name ON t TYPE string;\n\
         DEFINE FIELD flex ON t FLEXIBLE TYPE object;\n\
         DEFINE FIELD flex.name ON t TYPE string;\n\
         DEFINE FIELD maybe ON t TYPE option<object>;\n\
         DEFINE FIELD maybe.name ON t TYPE string;\n\
         DEFINE FIELD shaped ON t TYPE { name: string };\n\
         DEFINE FIELD shaped.age ON t TYPE int;\n\
         DEFINE FIELD untyped ON t VALUE {};\n\
         DEFINE FIELD untyped.name ON t TYPE string;\n\
         DEFINE FIELD items ON t TYPE array;\n\
         DEFINE FIELD items[*].sku ON t TYPE string;"
            .into(),
    );

    let output = analyze_workspace(&workspace);

    assert_eq!(codes(&output, 1025), 0, "{:?}", output.diagnostics);
}

#[test]
fn record_field_referencing_undefined_table_fires_1001() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE t SCHEMAFULL;\n\
         DEFINE FIELD owner ON t TYPE record<ghost>;"
            .into(),
    );

    let output = analyze_workspace(&workspace);

    assert_eq!(codes(&output, 1001), 1);
    let finding = output
        .diagnostics
        .iter()
        .find(|f| f.code().number() == 1001)
        .expect("a 1001 finding");
    assert!(finding.message().contains("ghost"));
}

#[test]
fn record_field_referencing_defined_tables_is_clean() {
    let mut workspace = Workspace::default();
    // Defined targets, including through option/array/union wrappers, must
    // never fire 1001.
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE user SCHEMAFULL;\n\
         DEFINE TABLE org SCHEMAFULL;\n\
         DEFINE TABLE t SCHEMAFULL;\n\
         DEFINE FIELD owner ON t TYPE record<user>;\n\
         DEFINE FIELD maybe ON t TYPE option<record<user>>;\n\
         DEFINE FIELD many ON t TYPE array<record<user | org>>;"
            .into(),
    );

    let output = analyze_workspace(&workspace);

    assert_eq!(
        codes(&output, 1001),
        0,
        "unexpected: {:?}",
        output.diagnostics
    );
}

#[test]
fn record_field_may_forward_reference_a_table_defined_later() {
    // A schema is applied as a unit, so `record<bb>` above `DEFINE TABLE
    // bb` is valid SurrealQL. The existence check consults the
    // whole-workspace catalog, matching what its help text always claimed
    // ("no `DEFINE TABLE bb` exists in the workspace"). Both the
    // same-source and later-source orderings must be clean.
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE a SCHEMAFULL;\n\
         DEFINE FIELD b ON a TYPE record<bb>;\n\
         DEFINE TABLE bb SCHEMAFULL;"
            .into(),
    );

    let output = analyze_workspace(&workspace);

    assert_eq!(
        codes(&output, 1001),
        0,
        "unexpected: {:?}",
        output.diagnostics
    );
}

#[test]
fn a_forward_reference_fix_does_not_excuse_a_never_defined_target() {
    // The counterpart contract: order-independence must not weaken the
    // genuine case. `never_defined` exists nowhere, before or after.
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE a SCHEMAFULL;\n\
         DEFINE FIELD b ON a TYPE record<bb>;\n\
         DEFINE FIELD c ON a TYPE option<record<never_defined>>;\n\
         DEFINE TABLE bb SCHEMAFULL;"
            .into(),
    );

    let output = analyze_workspace(&workspace);

    assert_eq!(
        codes(&output, 1001),
        1,
        "unexpected: {:?}",
        output.diagnostics
    );
    let finding = output
        .diagnostics
        .iter()
        .find(|f| f.code().number() == 1001)
        .expect("a 1001 finding");
    assert!(
        finding.message().contains("never_defined"),
        "{}",
        finding.message()
    );
}

#[test]
fn default_violating_own_assert_fires_2037() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE t SCHEMAFULL;\n\
         DEFINE FIELD status ON t TYPE string \
             DEFAULT 'activ' ASSERT $value IN ['active', 'inactive'];"
            .into(),
    );

    let output = analyze_workspace(&workspace);

    assert_eq!(
        codes(&output, 2037),
        1,
        "unexpected: {:?}",
        output.diagnostics
    );
}

#[test]
fn default_satisfying_own_assert_is_clean() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE t SCHEMAFULL;\n\
         DEFINE FIELD status ON t TYPE string \
             DEFAULT 'active' ASSERT $value IN ['active', 'inactive'];\n\
         DEFINE FIELD score ON t TYPE int DEFAULT 5 ASSERT $value >= 0 AND $value <= 10;"
            .into(),
    );

    let output = analyze_workspace(&workspace);

    assert_eq!(
        codes(&output, 2037),
        0,
        "unexpected: {:?}",
        output.diagnostics
    );
}

#[test]
fn non_const_default_or_assert_does_not_fire_2037() {
    let mut workspace = Workspace::default();
    // A non-const DEFAULT (function call) and a non-foldable ASSERT term
    // must both BAIL — never guess.
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE t SCHEMAFULL;\n\
         DEFINE FIELD created ON t TYPE datetime \
             DEFAULT time::now() ASSERT $value < time::now();\n\
         DEFINE FIELD name ON t TYPE string \
             DEFAULT 'x' ASSERT string::len($value) > 0;"
            .into(),
    );

    let output = analyze_workspace(&workspace);

    assert_eq!(
        codes(&output, 2037),
        0,
        "unexpected: {:?}",
        output.diagnostics
    );
}

#[test]
fn unique_and_plain_index_over_the_same_fields_are_not_redundant() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE t SCHEMAFULL;\n\
         DEFINE FIELD code ON t TYPE string;\n\
         DEFINE INDEX by_code ON t FIELDS code;\n\
         DEFINE INDEX unique_code ON t FIELDS code UNIQUE;"
            .into(),
    );

    let output = analyze_workspace(&workspace);

    // A UNIQUE index and a plain index over the same field do different
    // work, so 1029 must not fire.
    assert_eq!(codes(&output, 1029), 0);
}

#[test]
fn two_plain_indexes_over_the_same_fields_are_redundant() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE t SCHEMAFULL;\n\
         DEFINE FIELD code ON t TYPE string;\n\
         DEFINE INDEX by_code ON t FIELDS code;\n\
         DEFINE INDEX also_code ON t FIELDS code;"
            .into(),
    );

    let output = analyze_workspace(&workspace);

    assert_eq!(codes(&output, 1029), 1);
}

#[test]
fn event_then_block_ending_in_let_is_statement_position_not_a_value_block() {
    let mut workspace = Workspace::default();
    // An event THEN body is in statement position; a block ending in LET
    // is fine there, so the value-block lint (4017) must not fire.
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE t SCHEMAFULL;\n\
         DEFINE FIELD name ON t TYPE string;\n\
         DEFINE EVENT ev ON t WHEN $event = 'CREATE' THEN {\n\
             LET $x = CREATE t SET name = 'a';\n\
         };"
        .into(),
    );

    let output = analyze_workspace(&workspace);

    assert_eq!(codes(&output, 4017), 0);
}

#[test]
fn analyze_workspace_indexes_define_table_relation_metadata() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;".into(),
    );

    let output = analyze_workspace(&workspace);
    let relation = output.schema.tables["likes"]
        .relation
        .as_ref()
        .expect("likes is indexed as relation table");

    assert_eq!(relation.in_tables, vec!["person"]);
    assert_eq!(relation.out_tables, vec!["post"]);
}

#[test]
fn analyze_workspace_indexes_schemafull_field_declarations() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD profile.name ON person TYPE string;".into(),
    );

    let output = analyze_workspace(&workspace);

    let fields = &output.schema.tables["person"].fields;
    assert_eq!(fields.len(), 1);
    let field = &fields["profile.name"];
    assert_eq!(field.path, vec!["profile", "name"]);
    assert_eq!(field.table, "person");
    assert_eq!(field.kind, Some(Kind::String));
    assert_eq!(field.source, source);
    assert_eq!(field.name_span.range().start(), 45);
    assert_eq!(field.name_span.range().end(), 57);
    assert_eq!(field.table_span.range().start(), 61);
    assert_eq!(field.table_span.range().end(), 67);
}

#[test]
fn analyze_workspace_validates_define_index_table_and_fields() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD profile.email ON person TYPE string;\nDEFINE INDEX by_name ON TABLE person FIELDS name, profile.email;".into(),
    );

    let output = analyze_workspace(&workspace);

    assert!(output
        .diagnostics
        .iter()
        .all(|finding| finding.code() != FindingCode::schema(1002)));
    assert!(output
        .diagnostics
        .iter()
        .all(|finding| finding.code() != FindingCode::schema(1002)));
}

#[test]
fn analyze_workspace_reports_define_index_unknown_table_and_fields() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE INDEX by_email ON TABLE person FIELDS email;\nDEFINE INDEX missing_table_idx ON TABLE ghost FIELDS name;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| matches!(finding.code().number(), 1001 | 1002))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(
        messages,
        vec![
            "`person` has no field `email` (used by index `by_email`)",
            "index `missing_table_idx` is defined on `ghost`, which is not a defined table",
        ]
    );
}

#[test]
fn analyze_workspace_validates_define_event_table_and_when_fields() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE EVENT adult ON TABLE person WHEN $event.age >= 18 THEN { UPDATE person SET age = $event.age; };".into(),
    );

    let output = analyze_workspace(&workspace);

    assert!(output
        .diagnostics
        .iter()
        .all(|finding| finding.code() != FindingCode::schema(1002)));
    assert!(output
        .diagnostics
        .iter()
        .all(|finding| finding.code() != FindingCode::schema(1002)));
}

#[test]
fn analyze_workspace_reports_define_event_unknown_table_and_when_fields() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE EVENT bad_field ON TABLE person WHEN $event.missing >= 18 THEN { RETURN true; };\nDEFINE EVENT bad_table ON TABLE ghost WHEN $event.age > 18 THEN { RETURN true; };".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| matches!(finding.code().number(), 1001 | 1002))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(
        messages,
        vec![
            "event `bad_field` references unknown field `missing` on table `person`",
            "event `bad_table` targets unknown table `ghost`",
        ]
    );
}

#[test]
fn analyze_workspace_validates_rebuild_and_remove_index_targets() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE INDEX by_name ON TABLE person FIELDS name;\nREBUILD INDEX by_name ON TABLE person;\nREMOVE INDEX by_name ON TABLE person;\nREBUILD INDEX missing ON TABLE person;\nREMOVE INDEX by_name ON TABLE ghost;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code().number() == 1012)
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(
        messages,
        vec![
            "`person` has no index `missing` (REBUILD)",
            "REMOVE targets `ghost`, which is not a defined table",
        ]
    );
}

#[test]
fn analyze_workspace_validates_remove_table_and_field_targets() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD profile.email ON person TYPE string;\nREMOVE FIELD name ON TABLE person;\nREMOVE FIELD profile.email ON person;\nREMOVE TABLE ghost;\nREMOVE FIELD missing ON TABLE person;\nREMOVE FIELD name ON TABLE ghost;\nREMOVE TABLE person;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| matches!(finding.code(), code if code == FindingCode::schema(1021) || code == FindingCode::schema(1021)))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(
        messages,
        vec![
            "REMOVE TABLE `ghost` targets a table that doesn't exist",
            "`person` has no field `missing` to remove",
            "REMOVE FIELD `name` targets `ghost`, which is not a defined table",
        ]
    );
}

#[test]
fn analyze_workspace_validates_alter_table_targets() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE person;\nALTER TABLE person DROP;\nALTER TABLE ghost DROP;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1001))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(messages, vec!["`ghost` is not a defined table"]);
}

#[test]
fn analyze_workspace_reports_fields_on_unknown_tables() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE FIELD name ON person TYPE string;".into(),
    );

    let output = analyze_workspace(&workspace);

    // A `DEFINE FIELD ... ON` a never-`DEFINE`d table is valid: the table
    // exists schemaless. No unknown-table diagnostic fires, and no
    // `DEFINE TABLE` was seen so the catalog stays empty.
    assert!(output.schema.tables.is_empty());
    assert!(output
        .diagnostics
        .iter()
        .all(|finding| finding.code() != FindingCode::schema(1001)));
}

#[test]
fn analyze_workspace_resolves_structured_field_types() {
    // Parameterized/union/option types resolve to real kinds instead
    // of degrading to UnsupportedSyntax.
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE person;\nDEFINE FIELD tags ON person TYPE array<string>;\nDEFINE FIELD age ON person TYPE option<int>;\nDEFINE FIELD status ON person TYPE 'active' | 'inactive';".into(),
    );

    let output = analyze_workspace(&workspace);

    let fields = &output.schema.tables["person"].fields;
    assert_eq!(
        fields["tags"].kind,
        Some(Kind::Array(Box::new(Kind::String), None))
    );
    assert_eq!(
        fields["age"].kind,
        Some(Kind::Either(vec![Kind::None, Kind::Int]))
    );
    assert_eq!(
        fields["status"].kind,
        Some(Kind::Either(vec![
            Kind::Literal(KindLiteral::String("active".into())),
            Kind::Literal(KindLiteral::String("inactive".into())),
        ]))
    );
    assert!(output
        .diagnostics
        .iter()
        .all(|finding| finding.code() != FindingCode::param(6003)));
}

#[test]
fn analyze_workspace_marks_unsupported_field_type_syntax_as_partial_analysis() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE person;\nDEFINE FIELD shape ON person TYPE geometry<blob>;".into(),
    );

    let output = analyze_workspace(&workspace);

    let field = &output.schema.tables["person"].fields["shape"];
    assert!(field.kind.is_none());
    assert_eq!(
        field.partial,
        vec![PartialReason::UnsupportedSyntax("geometry<...>".into())]
    );
    let partial: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::param(6003))
        .collect();
    assert_eq!(partial.len(), 1);
}

#[test]
fn analyze_workspace_validates_select_table_references_against_schema() {
    let mut workspace = Workspace::default();
    let query = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nSELECT * FROM company;".into(),
    );

    let output = analyze_workspace(&workspace);

    let unknown_tables: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1001))
        .collect();
    assert_eq!(unknown_tables.len(), 1);
    assert_eq!(
        unknown_tables[0].message(),
        "`company` is not a defined table"
    );
    assert_eq!(unknown_tables[0].span().source(), &query);
    assert_eq!(unknown_tables[0].span().range().start(), 35);
    assert_eq!(unknown_tables[0].span().range().end(), 42);
    // The unknown table (1001) plus the two opt-in `SELECT *`/whole-table
    // lints (7014/7015, allow-by-default but emitted as raw findings).
    assert_eq!(output.sources[&query].diagnostics.len(), 3);
}

#[test]
fn analyze_workspace_allows_known_basic_table_references() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nSELECT * FROM person;\nCREATE person;\nUPDATE person SET name = 'A';\nDELETE person;".into(),
    );

    let output = analyze_workspace(&workspace);

    assert!(output
        .diagnostics
        .iter()
        .all(|finding| finding.code() != FindingCode::schema(1002)));
}

#[test]
fn analyze_workspace_treats_removed_table_as_absent_for_later_references() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nREMOVE TABLE person;\nUPDATE person SET name = 'Ada';".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1001))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(messages, vec!["`person` is not a defined table"]);
    assert!(output.schema.table("person").is_none());
}

#[test]
fn analyze_workspace_does_not_use_later_table_definitions_for_earlier_statements() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "UPDATE person SET name = 'Ada';\nDEFINE TABLE person;\nUPDATE person SET name = 'Grace';"
            .into(),
    );

    let output = analyze_workspace(&workspace);
    let unknown_tables: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1001))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(unknown_tables, vec!["`person` is not a defined table"]);
    assert!(output.schema.table("person").is_some());
}

#[test]
fn analyze_workspace_applies_field_definitions_and_removals_in_source_order() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person SCHEMAFULL;\nUPDATE person SET name = 'Ada';\nDEFINE FIELD name ON person TYPE int;\nUPDATE person SET name = 'Grace';\nREMOVE FIELD name ON person;\nUPDATE person SET name = 'Hedy';".into(),
    );

    let output = analyze_workspace(&workspace);
    let type_mismatches: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::type_error(2001))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(
        type_mismatches,
        vec!["`name` is declared `int`, but this value is `'Grace'`".to_string()]
    );
    assert!(output
        .schema
        .field("person", &FieldPath::parse("name"))
        .is_none());
}

#[test]
fn analyze_workspace_overwrite_replaces_definition_for_downstream_statements() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE user;\nDEFINE TABLE org;\nDEFINE TABLE person TYPE RELATION IN user OUT org;\nRELATE org:acme->person->user:drew;\nDEFINE TABLE OVERWRITE person TYPE RELATION IN org OUT user;\nRELATE org:acme->person->user:drew;".into(),
    );

    let output = analyze_workspace(&workspace);
    let endpoint_messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::graph(3002))
        .map(|finding| finding.message().to_string())
        .collect();

    // The first RELATE (before OVERWRITE) contradicts the declared
    // shape; the second (after) matches the overwritten relation.
    assert_eq!(
        endpoint_messages,
        vec![
            "relation `person` connects `user`->`person`->`org`, but this RELATE writes `org`->`person`->`user`",
        ]
    );
    let relation = output
        .schema
        .table("person")
        .and_then(|table| table.relation.as_ref())
        .unwrap();
    assert_eq!(relation.in_tables, vec!["org".to_string()]);
    assert_eq!(relation.out_tables, vec!["user".to_string()]);
}

#[test]
fn analyze_workspace_validates_create_update_and_delete_table_references() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "CREATE ghost;\nUPDATE phantom SET seen = true;\nDELETE missing;".into(),
    );

    let output = analyze_workspace(&workspace);

    // CREATE and DELETE create/act on schemaless tables on demand, so a
    // never-`DEFINE`d target is valid there. UPDATE only ever touches
    // existing rows, so its unknown target still reports.
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1001))
        .map(|finding| finding.message().to_string())
        .collect();
    assert_eq!(messages, vec!["`phantom` is not a defined table"]);
}

#[test]
fn analyze_workspace_validates_table_references_for_non_select_statement_forms() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "UPSERT ghost SET seen = true;\nINSERT INTO phantom { seen: true };\nLIVE SELECT * FROM missing;\nALTER TABLE shadow SCHEMAFULL;\nREMOVE TABLE stale;\nREBUILD INDEX by_name ON TABLE absent;\nSHOW CHANGES FOR TABLE vanished;\nINFO FOR TABLE hidden;\nINFO FOR TB obscured;".into(),
    );

    let output = analyze_workspace(&workspace);
    let unknown_tables: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code() == FindingCode::schema(1001))
        .map(|diagnostic| diagnostic.message().to_string())
        .collect();

    // UPSERT and INSERT write schemaless tables on demand, so `ghost` and
    // `phantom` are valid targets. The read/DDL forms below (LIVE SELECT,
    // ALTER, REBUILD, SHOW, INFO) still require the table to exist.
    assert_eq!(
        unknown_tables,
        vec![
            "`missing` is not a defined table",
            "`shadow` is not a defined table",
            "`absent` is not a defined table",
            "`vanished` is not a defined table",
            "`hidden` is not a defined table",
            "`obscured` is not a defined table",
        ]
    );
}

#[test]
fn analyze_workspace_default_auth_on_record_field_stays_clean() {
    // The workshop pattern (`organization.surql`): `DEFAULT $auth` on a
    // `record<account>` field. A field's DEFAULT/VALUE is a write-time
    // context whose session is not statically known, so the top-level
    // `$auth: option<record>` seed must not leak in and there make the
    // DEFAULT a false 2001 (option<record> vs record<account>).
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        concat!(
            "DEFINE TABLE account SCHEMAFULL;\n",
            "DEFINE TABLE organization SCHEMAFULL;\n",
            "DEFINE FIELD owner ON organization TYPE record<account> DEFAULT $auth;\n",
        )
        .into(),
    );
    let output = analyze_workspace(&workspace);
    assert_eq!(codes(&output, 2001), 0, "{:?}", output.diagnostics);
}

#[test]
fn analyze_workspace_value_and_default_auth_field_clauses_stay_clean() {
    // Other workshop field-clause shapes: `VALUE $auth` on a `record<T>`
    // field, and `DEFAULT ALWAYS $auth` on an `option<record<...>>` field.
    // None may raise a type mismatch from the session seed.
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        concat!(
            "DEFINE TABLE account SCHEMAFULL;\n",
            "DEFINE TABLE team SCHEMAFULL;\n",
            "DEFINE TABLE invite SCHEMAFULL;\n",
            "DEFINE FIELD created_by ON invite TYPE record<account> VALUE $auth READONLY;\n",
            "DEFINE FIELD owner ON invite TYPE option<record<account | team>> DEFAULT ALWAYS $auth;\n",
        )
        .into(),
    );
    let output = analyze_workspace(&workspace);
    assert_eq!(codes(&output, 2001), 0, "{:?}", output.diagnostics);
    assert_eq!(codes(&output, 2005), 0, "{:?}", output.diagnostics);
}

#[test]
fn analyze_workspace_checks_define_field_clauses() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        concat!(
            "DEFINE TABLE person SCHEMAFULL;\n",
            "DEFINE FIELD age ON person TYPE int DEFAULT 'young';\n",
            "DEFINE FIELD score ON person TYPE int ASSERT $value + 1;\n",
            "DEFINE FIELD ratio ON person TYPE int ASSERT $value > 'high';\n",
            "DEFINE FIELD created ON person TYPE datetime VALUE time::now() READONLY;\n",
            "DEFINE FIELD synced ON person TYPE bool VALUE http::get('https://x.test');\n",
            "UPDATE person SET created = time::now();\n",
            "UPDATE person SET synced = true;\n",
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
        // DEFAULT must inhabit the declared type.
        (
            "E2001",
            "`age`'s value is `'young'`, but the field is declared `int`",
        ),
        // ASSERT is a condition; `$value` carries the declared kind.
        ("E2005", "this ASSERT is a `int`, not a `bool`"),
        ("E2004", "`>` can't combine a `int` and a `string`"),
        // READONLY blocks non-creation writes; computed fields warn.
        ("E2025", "`created` can't be changed after creation"),
        ("E2026", "this write to `synced` is discarded"),
        ("L7012", "`http::get` runs on every write to this row"),
    ] {
        assert!(
            messages.iter().any(|(c, m)| c == code && m == message),
            "missing {code}: {message}\nhave: {messages:#?}"
        );
    }
}

#[test]
fn analyze_workspace_reports_schemaless_table_when_typed_tables_exist() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE TABLE bare;\nSELECT * FROM bare;".into(),
    );

    let output = analyze_workspace(&workspace);
    let diagnostics = &output.sources[&source].diagnostics;
    assert_no_syntax_findings(diagnostics);

    let messages: Vec<_> = diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::lint(7008))
        .map(|finding| finding.message().to_string())
        .collect();
    assert_eq!(
        messages,
        vec!["`bare` has no declared fields, so field-level checks are skipped"]
    );
}
