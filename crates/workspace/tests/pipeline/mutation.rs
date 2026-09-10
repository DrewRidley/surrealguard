//! Mutation contracts: CREATE/UPDATE/UPSERT/INSERT/RELATE payload field
//! validation and assignability, tuple INSERT rows, WHERE fields, and
//! RETURN clauses.

use std::collections::BTreeMap;

use surrealdb_types::{Kind, KindLiteral};
use surrealguard_diagnostics::FindingCode;
use surrealguard_workspace::{analyze_workspace, Workspace};

use crate::support::codes;

// ---- INSERT … ON DUPLICATE KEY UPDATE: the rows AND the assignments ----

#[test]
fn insert_on_duplicate_key_update_checks_the_rows_and_the_assignments() {
    let schema = "DEFINE TABLE person SCHEMAFULL;\n\
                  DEFINE FIELD name ON person TYPE string;\n\
                  DEFINE FIELD age ON person TYPE int;\n\
                  DEFINE FIELD created ON person TYPE datetime READONLY DEFAULT time::now();";
    let mut workspace = Workspace::default();
    workspace.add_virtual_source("schema".into(), schema.into());
    workspace.add_virtual_source(
        "query".into(),
        "INSERT INTO person { name: 'Ada' } \
         ON DUPLICATE KEY UPDATE bogus = 1, age = 'old', created = time::now();"
            .into(),
    );
    let output = analyze_workspace(&workspace);
    // The row payload is still a row: `age` is required and missing.
    assert_eq!(codes(&output, 2034), 1, "{:?}", output.diagnostics);
    // The assignments are field writes to an existing row: an unknown
    // field, a wrong kind, and a READONLY field are each reported.
    assert_eq!(codes(&output, 1002), 1, "{:?}", output.diagnostics);
    assert_eq!(codes(&output, 2001), 1, "{:?}", output.diagnostics);
    assert_eq!(codes(&output, 2025), 1, "{:?}", output.diagnostics);

    let mut workspace = Workspace::default();
    workspace.add_virtual_source("schema".into(), schema.into());
    workspace.add_virtual_source(
        "query".into(),
        "INSERT INTO person { name: 'Ada', age: 1 } ON DUPLICATE KEY UPDATE age += 1;".into(),
    );
    let output = analyze_workspace(&workspace);
    assert!(
        output.diagnostics.is_empty(),
        "a well-formed upsert is silent: {:?}",
        output.diagnostics
    );
}

const ADDRESS_SCHEMA: &str = "DEFINE TABLE org SCHEMAFULL;\n\
     DEFINE FIELD address ON org TYPE {\n\
         line1: string,\n\
         line2: option<string>,\n\
         city: string,\n\
         country: string\n\
     };\n\
     DEFINE FIELD nested ON org TYPE { inner: { code: int } };";

#[test]
fn a_param_inside_an_object_literal_is_constrained_by_the_declared_subfield() {
    // `SET field = $param` constrained the param; the nested case did not,
    // so the params stayed `any`, `{ line1: any }` failed assignability
    // against `{ line1: string }`, and a valid write became an
    // error-severity 2001 that aborts `generate`.
    let mut workspace = Workspace::default();
    workspace.add_virtual_source("schema".into(), ADDRESS_SCHEMA.into());
    let query = workspace.add_virtual_source(
        "query".into(),
        "CREATE org SET address = { line1: $line1, line2: NONE, city: $city, country: 'US' },\n\
             nested = { inner: { code: $code } };"
            .into(),
    );

    let output = analyze_workspace(&workspace);

    assert_eq!(codes(&output, 2001), 0, "{:?}", output.diagnostics);
    let params: BTreeMap<String, Option<Kind>> = output.sources[&query]
        .inferred_params
        .iter()
        .map(|param| (param.name.clone(), param.kind.clone()))
        .collect();
    assert_eq!(params["line1"], Some(Kind::String));
    assert_eq!(params["city"], Some(Kind::String));
    // Constraining recurses, so a param two levels down is typed too.
    assert_eq!(params["code"], Some(Kind::Int));
}

#[test]
fn a_wrong_write_inside_an_object_literal_is_still_a_type_error() {
    // The counterpart: standing the declared kind in for a param must not
    // make the object comparison permissive. A missing required subfield,
    // a wrong-kinded value, and an undeclared key all still fail.
    let mut workspace = Workspace::default();
    workspace.add_virtual_source("schema".into(), ADDRESS_SCHEMA.into());
    workspace.add_virtual_source(
        "query".into(),
        "CREATE org SET address = { line1: $line1, city: $city };\n\
         CREATE org SET address = { line1: 1, city: $city, country: $country };\n\
         CREATE org SET address = { line1: $line1, city: $city, country: $country, bogus: 1 };"
            .into(),
    );

    let output = analyze_workspace(&workspace);

    assert_eq!(codes(&output, 2001), 3, "{:?}", output.diagnostics);
}

#[test]
fn analyze_workspace_uses_let_variable_kind_for_mutation_assignability() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nLET $age = 42;\nCREATE person SET age = $age;".into(),
    );

    let output = analyze_workspace(&workspace);
    let source_output = &output.sources[&source];

    assert!(source_output.inferred_params.is_empty());
    assert!(source_output
        .diagnostics
        .iter()
        .all(|finding| finding.code().to_string() != "E2001"));
}

#[test]
fn analyze_workspace_reports_let_variable_kind_mismatch_for_mutation_assignability() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nLET $age = 'old';\nCREATE person SET age = $age;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    assert!(messages.iter().any(|(code, message)| {
        code == "E2001" && message == "`age` is declared `int`, but this value is `'old'`"
    }));
}

#[test]
fn analyze_workspace_reports_branch_local_let_mismatch_for_mutation_assignability() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nIF true { LET $age = 'old'; CREATE person SET age = $age; } ELSE { RETURN 0; };".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    assert!(messages.iter().any(|(code, message)| {
        code == "E2001" && message == "`age` is declared `int`, but this value is `'old'`"
    }));
}

#[test]
fn analyze_workspace_uses_dependent_let_variable_kind_for_mutation_assignability() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nLET $age = 42;\nLET $next = $age + 1;\nCREATE person SET age = $next;".into(),
    );

    let output = analyze_workspace(&workspace);
    let source_output = &output.sources[&source];

    assert!(source_output.inferred_params.is_empty());
    assert!(source_output
        .diagnostics
        .iter()
        .all(|finding| finding.code().to_string() != "E2001"));
}

#[test]
fn analyze_workspace_reports_dependent_let_variable_kind_mismatch() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nLET $name = 'Drew';\nLET $excited = $name + '!';\nCREATE person SET age = $excited;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    assert!(messages.iter().any(|(code, message)| {
        code == "E2001" && message == "`age` is declared `int`, but this value is `string`"
    }));
}

#[test]
fn analyze_workspace_reports_unknown_mutation_assignment_fields() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nCREATE person SET nickname = 'Ada', name = 'Ada';\nUPDATE person SET handle = 'ada', name = 'Ada';\nUPSERT person SET alias = 'ada', name = 'Ada';".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1002))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(
        messages,
        vec![
            "`person` has no field `nickname`",
            "`person` has no field `handle`",
            "`person` has no field `alias`",
        ]
    );
}

#[test]
fn analyze_workspace_reports_unknown_object_mutation_fields() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD profile.email ON person TYPE string;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD created_at ON likes TYPE datetime;\nCREATE person CONTENT { nickname: 'Ada', profile: { phone: '555' }, name: 'Ada' };\nINSERT INTO person { handle: 'ada', name: 'Ada' };\nINSERT INTO person (alias, name) VALUES ('ada', 'Ada');\nUPDATE person MERGE { stale: true, name: 'Ada' };\nUPSERT person REPLACE { missing: true, name: 'Ada' };\nRELATE person:one->likes->post:one CONTENT { missing_since: time::now(), created_at: time::now() };".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| matches!(finding.code().number(), 1002..=1011))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(
        messages,
        vec![
            "`person` has no field `nickname`",
            "`person` has no field `profile.phone`",
            "`person` has no field `handle`",
            "`person` has no field `alias`",
            "`person` has no field `stale`",
            "`person` has no field `missing`",
            "`likes` has no field `missing_since`",
        ]
    );
}

#[test]
fn analyze_workspace_reports_mutation_value_type_mismatches() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD name ON person TYPE string;\nCREATE person SET age = 'old', name = 'Ada';\nUPDATE person MERGE { age: 'old', name: 'Ada' };\nUPSERT person CONTENT { age: 'old', name: 'Ada' };\nINSERT INTO person (age, name) VALUES ('old', 'Ada');".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| matches!(finding.code().number(), 2001..=2003))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(
        messages,
        vec![
            "`age` is declared `int`, but this value is `'old'`",
            "`age` is declared `int`, but this value is `'old'`",
            "`age` is declared `int`, but this value is `'old'`",
            "`age` is declared `int`, but this value is `'old'`",
        ]
    );
}

#[test]
fn analyze_workspace_skips_type_mismatch_for_dynamic_mutation_params() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nCREATE person SET age = $age;\nUPDATE person MERGE { age: $age };".into(),
    );

    let output = analyze_workspace(&workspace);
    let type_errors: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::type_error(2001))
        .collect();

    assert!(type_errors.is_empty());
}

#[test]
fn analyze_workspace_reports_nested_and_relate_payload_type_mismatches() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD profile.email ON person TYPE string;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD created_at ON likes TYPE datetime;\nUPDATE person MERGE { profile: { email: 10 } };\nRELATE person:one->likes->post:one CONTENT { created_at: 'yesterday' };".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::type_error(2001))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(
        messages,
        vec![
            "`profile.email` is declared `string`, but this value is `10`",
            "`created_at` is declared `datetime`, but this value is `'yesterday'`",
        ]
    );
}

#[test]
fn analyze_workspace_reports_tuple_insert_arity_mismatches() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD name ON person TYPE string;\nINSERT INTO person (age, name) VALUES (1);\nINSERT INTO person (age, name) VALUES (1, 'Ada', true);".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::statement(4004))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(
        messages,
        vec![
            "this INSERT row has 1 value but 2 columns",
            "this INSERT row has 3 values but 2 columns",
        ]
    );
}

#[test]
fn analyze_workspace_checks_tuple_insert_values_across_flattened_rows() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD name ON person TYPE string;\nINSERT INTO person (age, name) VALUES (1, 'Ada'), ('old', 'Grace');".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::type_error(2001))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(
        messages,
        vec!["`age` is declared `int`, but this value is `'old'`"]
    );
}

#[test]
fn analyze_workspace_reports_unknown_mutation_where_fields() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD age ON person TYPE int;\nUPDATE person SET name = 'Ada' WHERE missing > 0 AND age > 18;\nUPSERT person SET name = 'Ada' WHERE ghost = true AND name = 'Ada';\nDELETE person WHERE stale = true AND age < 99;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1002))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(
        messages,
        vec![
            "`person` has no field `missing`",
            "`person` has no field `ghost`",
            "`person` has no field `stale`",
        ]
    );
    assert_eq!(output.sources[&source].diagnostics.len(), 3);
}

#[test]
fn analyze_workspace_reports_none_assignment_and_bad_negation() {
    // (2014 negation coverage waits on the grammar: prefix `-` only
    // parses on numeric literals today.)
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD retired_at ON person TYPE option<datetime>;\nUPDATE person SET age = NONE, retired_at = NONE;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    // NONE into a non-optional field violates the one assignability
    // contract (2001); into option<datetime> it is fine.
    assert!(messages.iter().any(|(code, message)| {
        code == "E2001" && message == "`age` is not optional, so it can't be set to none"
    }));
    assert!(!messages
        .iter()
        .any(|(_, message)| message.contains("retired_at")));
}

#[test]
fn analyze_workspace_reports_unknown_mutation_return_fields() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nUPDATE person RETURN nickname, profile.phone;".into(),
    );

    let output = analyze_workspace(&workspace);
    let unknown_fields: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1002))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(
        unknown_fields,
        vec![
            "`person` has no field `nickname`".to_string(),
            "`person` has no field `profile.phone`".to_string(),
        ]
    );
}

#[test]
fn analyze_workspace_validates_mutation_return_alias_expressions_without_alias_field_false_positive(
) {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nUPDATE person RETURN name AS label, missing AS projected_missing;".into(),
    );

    let output = analyze_workspace(&workspace);
    let unknown_fields: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1002))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(
        unknown_fields,
        vec!["`person` has no field `missing`".to_string()]
    );
}

#[test]
fn analyze_workspace_infers_mutation_return_expression_shape_from_let_env() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nLET $bonus = 1;\nUPDATE person SET age = 42 RETURN $bonus + 1 AS next_bonus;".into(),
    );

    let output = analyze_workspace(&workspace);
    let update = output.sources[&source]
        .statements
        .iter()
        .find(|statement| statement.kind == "update")
        .expect("update statement exists");

    let Some(Kind::Array(element, _)) = &update.response_kind else {
        panic!("expected array kind, got {:?}", update.response_kind);
    };
    let Kind::Literal(KindLiteral::Object(fields)) = element.as_ref() else {
        panic!("expected object literal element, got {element:?}");
    };
    assert_eq!(
        fields.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["next_bonus"]
    );
    assert_eq!(fields["next_bonus"], Kind::Int);
}
