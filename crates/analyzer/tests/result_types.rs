//! Result type inference tests.
//!
//! Tests that every statement returns the correct `Kind` for its result type,
//! accounting for all modifiers (VALUE, ONLY, OMIT, FETCH, RETURN clause, etc.)

use std::collections::BTreeMap;
use surrealguard_analyzer::types::{Kind, Literal};
use surrealguard_analyzer::{parse, Context};

// ── Test Helpers ───────────────────────────────────────────────

/// Parse source, extract schema, then find and analyze the last statement
/// of the given kind, returning its result type.
fn select_type(source: &str) -> Kind {
    statement_type(source, "select_statement")
}

fn create_type(source: &str) -> Kind {
    statement_type(source, "create_statement")
}

fn update_type(source: &str) -> Kind {
    statement_type(source, "update_statement")
}

fn upsert_type(source: &str) -> Kind {
    statement_type(source, "upsert_statement")
}

fn insert_type(source: &str) -> Kind {
    statement_type(source, "insert_statement")
}

fn delete_type(source: &str) -> Kind {
    statement_type(source, "delete_statement")
}

fn relate_type(source: &str) -> Kind {
    statement_type(source, "relate_statement")
}

fn statement_type(source: &str, kind: &str) -> Kind {
    let tree = parse(source).unwrap();
    let root = tree.root_node();

    let mut ctx = Context::new();
    surrealguard_analyzer::schema::extract_schema(&root, source, &mut ctx);

    // Find the last statement of the given kind
    let stmts = surrealguard_analyzer::parser::find_all(&root, kind);
    let stmt = stmts.last().expect(&format!("no {} found in source", kind));

    surrealguard_analyzer::statements::analyze_statement(stmt, source, &mut ctx)
}

/// Helper: build an object type from field name→kind pairs.
fn obj(fields: &[(&str, Kind)]) -> Kind {
    let map: BTreeMap<String, Kind> = fields
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    Kind::Literal(Literal::Object(map))
}

/// Helper: wrap a type in array.
fn arr(inner: Kind) -> Kind {
    Kind::Array(Box::new(inner), None)
}

/// Assert two Kinds are equal, with a readable message.
fn assert_kind(actual: &Kind, expected: &Kind, msg: &str) {
    assert_eq!(
        actual, expected,
        "{}\n  expected: {:?}\n  actual:   {:?}",
        msg, expected, actual
    );
}

// ── Schema used by most tests ──────────────────────────────────

const USER_SCHEMA: &str = r#"
    DEFINE TABLE user SCHEMAFULL;
    DEFINE FIELD name ON user TYPE string;
    DEFINE FIELD age ON user TYPE int;
    DEFINE FIELD email ON user TYPE string;
"#;

const USER_WITH_RECORD: &str = r#"
    DEFINE TABLE user SCHEMAFULL;
    DEFINE FIELD name ON user TYPE string;
    DEFINE FIELD age ON user TYPE int;
    DEFINE TABLE organization SCHEMAFULL;
    DEFINE FIELD title ON organization TYPE string;
    DEFINE FIELD size ON organization TYPE int;
    DEFINE FIELD org ON user TYPE record<organization>;
"#;

const RELATION_SCHEMA: &str = r#"
    DEFINE TABLE user SCHEMAFULL;
    DEFINE FIELD name ON user TYPE string;
    DEFINE TABLE post SCHEMAFULL;
    DEFINE FIELD title ON post TYPE string;
    DEFINE TABLE wrote TYPE RELATION FROM user TO post;
    DEFINE FIELD created_at ON wrote TYPE datetime;
"#;

// ════════════════════════════════════════════════════════════════
// SELECT result types
// ════════════════════════════════════════════════════════════════

#[test]
fn select_star_returns_array_of_full_object() {
    let src = format!("{}\nSELECT * FROM user;", USER_SCHEMA);
    let typ = select_type(&src);
    let expected = arr(obj(&[
        ("age", Kind::Int),
        ("email", Kind::String),
        ("name", Kind::String),
    ]));
    assert_kind(&typ, &expected, "SELECT * should return array of full table object");
}

#[test]
fn select_specific_fields_returns_projected_object() {
    let src = format!("{}\nSELECT name, age FROM user;", USER_SCHEMA);
    let typ = select_type(&src);
    let expected = arr(obj(&[("age", Kind::Int), ("name", Kind::String)]));
    assert_kind(
        &typ,
        &expected,
        "SELECT name, age should return array of {name: string, age: int}",
    );
}

#[test]
fn select_single_field_returns_object_with_one_field() {
    let src = format!("{}\nSELECT name FROM user;", USER_SCHEMA);
    let typ = select_type(&src);
    let expected = arr(obj(&[("name", Kind::String)]));
    assert_kind(
        &typ,
        &expected,
        "SELECT name should return array of {name: string}",
    );
}

#[test]
fn select_value_returns_array_of_field_type() {
    let src = format!("{}\nSELECT VALUE name FROM user;", USER_SCHEMA);
    let typ = select_type(&src);
    let expected = arr(Kind::String);
    assert_kind(
        &typ,
        &expected,
        "SELECT VALUE name should return array<string>",
    );
}

#[test]
fn select_value_int_field() {
    let src = format!("{}\nSELECT VALUE age FROM user;", USER_SCHEMA);
    let typ = select_type(&src);
    let expected = arr(Kind::Int);
    assert_kind(
        &typ,
        &expected,
        "SELECT VALUE age should return array<int>",
    );
}

#[test]
fn select_only_returns_single_record() {
    let src = format!("{}\nSELECT * FROM ONLY user:1;", USER_SCHEMA);
    let typ = select_type(&src);
    let expected = obj(&[
        ("age", Kind::Int),
        ("email", Kind::String),
        ("name", Kind::String),
    ]);
    assert_kind(
        &typ,
        &expected,
        "SELECT * FROM ONLY should return single object, not array",
    );
}

#[test]
fn select_value_only_returns_scalar() {
    let src = format!("{}\nSELECT VALUE name FROM ONLY user:1;", USER_SCHEMA);
    let typ = select_type(&src);
    assert_kind(
        &typ,
        &Kind::String,
        "SELECT VALUE name FROM ONLY should return string (scalar, not array)",
    );
}

#[test]
fn select_omit_removes_field_from_result() {
    let src = format!("{}\nSELECT * OMIT email FROM user;", USER_SCHEMA);
    let typ = select_type(&src);
    let expected = arr(obj(&[("age", Kind::Int), ("name", Kind::String)]));
    assert_kind(
        &typ,
        &expected,
        "SELECT * OMIT email should exclude email from result type",
    );
}

#[test]
fn select_omit_multiple_fields() {
    let src = format!("{}\nSELECT * OMIT email, age FROM user;", USER_SCHEMA);
    let typ = select_type(&src);
    let expected = arr(obj(&[("name", Kind::String)]));
    assert_kind(
        &typ,
        &expected,
        "SELECT * OMIT email, age should leave only name",
    );
}

#[test]
fn select_alias_renames_field_in_result() {
    let src = format!("{}\nSELECT name AS username FROM user;", USER_SCHEMA);
    let typ = select_type(&src);
    let expected = arr(obj(&[("username", Kind::String)]));
    assert_kind(
        &typ,
        &expected,
        "SELECT name AS username should use alias in result type",
    );
}

#[test]
fn select_fetch_expands_record_link() {
    let src = format!(
        "{}\nSELECT name, org FROM user FETCH org;",
        USER_WITH_RECORD
    );
    let typ = select_type(&src);
    // After FETCH, org should be expanded from record<organization> to {title: string, size: int}
    let expected = arr(obj(&[
        ("name", Kind::String),
        (
            "org",
            obj(&[("size", Kind::Int), ("title", Kind::String)]),
        ),
    ]));
    assert_kind(
        &typ,
        &expected,
        "FETCH should expand record<organization> to full schema",
    );
}

#[test]
fn select_only_with_omit() {
    let src = format!("{}\nSELECT * OMIT email FROM ONLY user:1;", USER_SCHEMA);
    let typ = select_type(&src);
    let expected = obj(&[("age", Kind::Int), ("name", Kind::String)]);
    assert_kind(
        &typ,
        &expected,
        "ONLY + OMIT should return single object without omitted fields",
    );
}

// ════════════════════════════════════════════════════════════════
// CREATE result types
// ════════════════════════════════════════════════════════════════

#[test]
fn create_returns_array_of_table_type() {
    let src = format!("{}\nCREATE user SET name = 'John', age = 30, email = 'j@t.com';", USER_SCHEMA);
    let typ = create_type(&src);
    let expected = arr(obj(&[
        ("age", Kind::Int),
        ("email", Kind::String),
        ("name", Kind::String),
    ]));
    assert_kind(
        &typ,
        &expected,
        "CREATE should return array<table_schema>",
    );
}

#[test]
fn create_only_returns_single_record() {
    let src = format!(
        "{}\nCREATE ONLY user SET name = 'John', age = 30, email = 'j@t.com';",
        USER_SCHEMA
    );
    let typ = create_type(&src);
    let expected = obj(&[
        ("age", Kind::Int),
        ("email", Kind::String),
        ("name", Kind::String),
    ]);
    assert_kind(
        &typ,
        &expected,
        "CREATE ONLY should return single object, not array",
    );
}

#[test]
fn create_return_none() {
    let src = format!(
        "{}\nCREATE user SET name = 'John', age = 30, email = 'j@t.com' RETURN NONE;",
        USER_SCHEMA
    );
    let typ = create_type(&src);
    assert_kind(&typ, &Kind::Null, "CREATE ... RETURN NONE should return null");
}

#[test]
fn create_return_specific_fields() {
    let src = format!(
        "{}\nCREATE user SET name = 'John', age = 30, email = 'j@t.com' RETURN name, age;",
        USER_SCHEMA
    );
    let typ = create_type(&src);
    let expected = arr(obj(&[("age", Kind::Int), ("name", Kind::String)]));
    assert_kind(
        &typ,
        &expected,
        "CREATE ... RETURN name, age should return projected object",
    );
}

// ════════════════════════════════════════════════════════════════
// UPDATE result types
// ════════════════════════════════════════════════════════════════

#[test]
fn update_returns_array_of_table_type() {
    let src = format!("{}\nUPDATE user SET age = 31;", USER_SCHEMA);
    let typ = update_type(&src);
    let expected = arr(obj(&[
        ("age", Kind::Int),
        ("email", Kind::String),
        ("name", Kind::String),
    ]));
    assert_kind(
        &typ,
        &expected,
        "UPDATE should return array<table_schema>",
    );
}

#[test]
fn update_only_returns_single_record() {
    let src = format!("{}\nUPDATE ONLY user:1 SET age = 31;", USER_SCHEMA);
    let typ = update_type(&src);
    let expected = obj(&[
        ("age", Kind::Int),
        ("email", Kind::String),
        ("name", Kind::String),
    ]);
    assert_kind(
        &typ,
        &expected,
        "UPDATE ONLY should return single object",
    );
}

#[test]
fn update_return_none() {
    let src = format!("{}\nUPDATE user SET age = 31 RETURN NONE;", USER_SCHEMA);
    let typ = update_type(&src);
    assert_kind(&typ, &Kind::Null, "UPDATE ... RETURN NONE should return null");
}

#[test]
fn update_return_diff() {
    let src = format!("{}\nUPDATE user SET age = 31 RETURN DIFF;", USER_SCHEMA);
    let typ = update_type(&src);
    // RETURN DIFF returns an array of JSON Patch operations
    // Each op is: { op: string, path: string, value: any }
    match &typ {
        Kind::Array(inner, _) => {
            // The inner type should be an object (the diff)
            assert!(
                matches!(inner.as_ref(), Kind::Object | Kind::Literal(Literal::Object(_))),
                "RETURN DIFF should return array of objects, got {:?}",
                typ
            );
        }
        _ => panic!(
            "UPDATE RETURN DIFF should return an array, got {:?}",
            typ
        ),
    }
}

#[test]
fn update_return_specific_fields() {
    let src = format!("{}\nUPDATE user SET age = 31 RETURN name;", USER_SCHEMA);
    let typ = update_type(&src);
    let expected = arr(obj(&[("name", Kind::String)]));
    assert_kind(
        &typ,
        &expected,
        "UPDATE ... RETURN name should return projected object",
    );
}

// ════════════════════════════════════════════════════════════════
// DELETE result types
// ════════════════════════════════════════════════════════════════

#[test]
fn delete_returns_array_of_table_type() {
    let src = format!("{}\nDELETE user;", USER_SCHEMA);
    let typ = delete_type(&src);
    let expected = arr(obj(&[
        ("age", Kind::Int),
        ("email", Kind::String),
        ("name", Kind::String),
    ]));
    assert_kind(
        &typ,
        &expected,
        "DELETE should return array<table_schema>",
    );
}

#[test]
fn delete_return_none() {
    let src = format!("{}\nDELETE user RETURN NONE;", USER_SCHEMA);
    let typ = delete_type(&src);
    assert_kind(&typ, &Kind::Null, "DELETE ... RETURN NONE should return null");
}

#[test]
fn delete_only_returns_single() {
    let src = format!("{}\nDELETE ONLY user:1;", USER_SCHEMA);
    let typ = delete_type(&src);
    let expected = obj(&[
        ("age", Kind::Int),
        ("email", Kind::String),
        ("name", Kind::String),
    ]);
    assert_kind(
        &typ,
        &expected,
        "DELETE ONLY should return single object",
    );
}

// ════════════════════════════════════════════════════════════════
// INSERT result types
// ════════════════════════════════════════════════════════════════

#[test]
fn insert_returns_array_of_table_type() {
    let src = format!(
        "{}\nINSERT INTO user {{ name: 'John', age: 30, email: 'j@t.com' }};",
        USER_SCHEMA
    );
    let typ = insert_type(&src);
    let expected = arr(obj(&[
        ("age", Kind::Int),
        ("email", Kind::String),
        ("name", Kind::String),
    ]));
    assert_kind(
        &typ,
        &expected,
        "INSERT should return array<table_schema>",
    );
}

// ════════════════════════════════════════════════════════════════
// UPSERT result types
// ════════════════════════════════════════════════════════════════

#[test]
fn upsert_returns_array_of_table_type() {
    let src = format!("{}\nUPSERT user SET age = 31;", USER_SCHEMA);
    let typ = upsert_type(&src);
    let expected = arr(obj(&[
        ("age", Kind::Int),
        ("email", Kind::String),
        ("name", Kind::String),
    ]));
    assert_kind(
        &typ,
        &expected,
        "UPSERT should return array<table_schema>",
    );
}

// ════════════════════════════════════════════════════════════════
// RELATE result types
// ════════════════════════════════════════════════════════════════

#[test]
fn relate_returns_array_of_relation_type() {
    let src = format!(
        "{}\nRELATE user:1->wrote->post:1 SET created_at = d'2024-01-01';",
        RELATION_SCHEMA
    );
    let typ = relate_type(&src);
    let expected = arr(obj(&[("created_at", Kind::Datetime)]));
    assert_kind(
        &typ,
        &expected,
        "RELATE should return array<relation_table_schema>",
    );
}

// ════════════════════════════════════════════════════════════════
// Subquery / block result type propagation
// ════════════════════════════════════════════════════════════════

/// Helper: resolve the type of an expression node found by kind.
fn resolve_expr_type(source: &str, node_kind: &str) -> Kind {
    let tree = parse(source).unwrap();
    let root = tree.root_node();

    let mut ctx = Context::new();
    surrealguard_analyzer::schema::extract_schema(&root, source, &mut ctx);

    let nodes = surrealguard_analyzer::parser::find_all(&root, node_kind);
    let node = nodes.last().expect(&format!("no {} found in source", node_kind));

    surrealguard_analyzer::resolve::resolve_expr(node, source, &mut ctx, None)
}

/// Helper: analyze all statements, then look up a variable's inferred type.
fn variable_type_after(source: &str, var_name: &str) -> Kind {
    let tree = parse(source).unwrap();
    let root = tree.root_node();

    let mut ctx = Context::new();
    surrealguard_analyzer::schema::extract_schema(&root, source, &mut ctx);
    surrealguard_analyzer::statements::analyze_all(&root, source, &mut ctx);

    ctx.scope
        .lookup(var_name)
        .map(|b| b.typ.clone())
        .unwrap_or_else(|| panic!("variable {} not found in scope", var_name))
}

#[test]
fn subquery_create_only_propagates_type() {
    let src = format!(
        "{}\n(CREATE ONLY user SET name = 'John', age = 30, email = 'j@t.com');",
        USER_SCHEMA
    );
    let typ = resolve_expr_type(&src, "sub_query");
    let expected = obj(&[
        ("age", Kind::Int),
        ("email", Kind::String),
        ("name", Kind::String),
    ]);
    assert_kind(
        &typ,
        &expected,
        "Subquery (CREATE ONLY ...) should return single object type",
    );
}

#[test]
fn subquery_select_propagates_type() {
    let src = format!(
        "{}\n(SELECT name FROM user);",
        USER_SCHEMA
    );
    let typ = resolve_expr_type(&src, "sub_query");
    let expected = arr(obj(&[("name", Kind::String)]));
    assert_kind(
        &typ,
        &expected,
        "Subquery (SELECT name FROM user) should return array<{{name: string}}>",
    );
}

#[test]
fn let_with_subquery_create_only_infers_type() {
    let src = format!(
        "{}\nLET $user = (CREATE ONLY user SET name = 'John', age = 30, email = 'j@t.com');",
        USER_SCHEMA
    );
    let typ = variable_type_after(&src, "$user");
    let expected = obj(&[
        ("age", Kind::Int),
        ("email", Kind::String),
        ("name", Kind::String),
    ]);
    assert_kind(
        &typ,
        &expected,
        "LET $user = (CREATE ONLY ...) should infer single object type",
    );
}

#[test]
fn let_with_subquery_select_infers_type() {
    let src = format!(
        "{}\nLET $users = (SELECT * FROM user);",
        USER_SCHEMA
    );
    let typ = variable_type_after(&src, "$users");
    let expected = arr(obj(&[
        ("age", Kind::Int),
        ("email", Kind::String),
        ("name", Kind::String),
    ]));
    assert_kind(
        &typ,
        &expected,
        "LET $users = (SELECT * FROM user) should infer array<full_object>",
    );
}

#[test]
fn block_returns_last_expression_type() {
    // Use IF to get a block context — the block is the then-branch
    let src = "IF true { LET $x = 1; $x }";
    let typ = resolve_expr_type(&src, "block");
    // The last expression is $x which was bound to number (1 is parsed as number literal)
    assert_kind(
        &typ,
        &Kind::Number,
        "Block should return the type of its last expression",
    );
}

#[test]
fn block_returns_last_expression_string() {
    let src = "IF true { LET $x = 1; 'hello' }";
    let typ = resolve_expr_type(&src, "block");
    assert_kind(
        &typ,
        &Kind::String,
        "Block should return the type of the last expression (string)",
    );
}

// ════════════════════════════════════════════════════════════════
// IF/ELSE result types
// ════════════════════════════════════════════════════════════════

fn if_type(source: &str) -> Kind {
    statement_type(source, "if_statement")
}

#[test]
fn if_else_returns_unified_type_same_branches() {
    let src = "IF true { 42 } ELSE { 99 };";
    let typ = if_type(src);
    assert_kind(
        &typ,
        &Kind::Number,
        "IF/ELSE with both number branches should return number",
    );
}

#[test]
fn if_else_returns_either_for_different_branches() {
    let src = r#"IF true { 42 } ELSE { "hello" };"#;
    let typ = if_type(src);
    assert_kind(
        &typ,
        &Kind::Either(vec![Kind::Number, Kind::String]),
        "IF/ELSE with number and string branches should return number|string",
    );
}

#[test]
fn if_no_else_returns_branch_type() {
    let src = "IF true { 42 };";
    let typ = if_type(src);
    assert_kind(
        &typ,
        &Kind::Number,
        "IF without ELSE should return the then-branch type",
    );
}

#[test]
fn if_condition_not_bool_emits_warning() {
    let src = r#"IF "not_a_bool" { 1 };"#;
    let result = surrealguard_analyzer::analyze(src).unwrap();
    let has_warning = result.diagnostics.iter().any(|d| {
        d.code == surrealguard_analyzer::diagnostic::Code::TypeMismatch
            && d.message.contains("condition should be bool")
    });
    assert!(
        has_warning,
        "IF with non-bool condition should emit TypeMismatch warning, got: {:?}",
        result.diagnostics
    );
}

// ════════════════════════════════════════════════════════════════
// FOR loop iterable type checking
// ════════════════════════════════════════════════════════════════

#[test]
fn for_loop_non_array_iterable_emits_warning() {
    let src = r#"FOR $x IN 42 { $x };"#;
    let result = surrealguard_analyzer::analyze(src).unwrap();
    let has_warning = result.diagnostics.iter().any(|d| {
        d.code == surrealguard_analyzer::diagnostic::Code::TypeMismatch
            && d.message.contains("cannot iterate")
    });
    assert!(
        has_warning,
        "FOR with non-array iterable should emit TypeMismatch warning, got: {:?}",
        result.diagnostics
    );
}
