//! Control flow and narrowing: `LET` environments, `IF`/`ELSE` response
//! shapes and conditions, branch-local scope, and guard narrowing.

use surrealdb_types::Kind;
use surrealql_analyzer_workspace::{analyze_workspace, render_kind, Workspace};

use crate::support::codes;

#[test]
fn a_sentinel_guard_narrows_out_only_its_own_sentinel() {
    // `NONE` and `NULL` are distinct values in SurrealDB — verified on the
    // engine, `RETURN NULL = NONE` is `false` and `RETURN NONE IS NULL` is
    // `false`. So a NULL passes an `IF $x = NONE THEN THROW` guard and
    // reaches the code after it: the narrowed type is `string | null`, and
    // reporting `string` is a type the database can violate.
    //
    // Each row is (param type, guard, expected narrowed return).
    for (param, guard, expected) in [
        // The unsoundness: `= NONE` / `IS NONE` must keep the `null`.
        ("option<string | null>", "$x = NONE", "string | null"),
        ("option<string | null>", "$x IS NONE", "string | null"),
        // The mirror: `= NULL` eliminates only NULL, keeping `none`.
        ("option<string | null>", "$x = NULL", "option<string>"),
        ("option<string | null>", "$x IS NULL", "option<string>"),
        // A plain `option<T>` has no `null`, so it still narrows to `T`.
        ("option<string>", "$x = NONE", "string"),
        ("option<string>", "$x IS NONE", "string"),
    ] {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            format!(
                "DEFINE FUNCTION fn::probe($x: {param}) {{ \
                 IF {guard} THEN THROW 'x' END; RETURN $x; }};"
            ),
        );
        let query = workspace.add_virtual_source("query".into(), "RETURN fn::probe('a');".into());

        let output = analyze_workspace(&workspace);
        let rendered = render_kind(
            output.sources[&query]
                .response_kind
                .as_ref()
                .expect("response kind"),
        );
        assert_eq!(rendered, expected, "{param} guarded by `{guard}`");
    }
}

#[test]
fn if_expression_in_value_position_types_as_the_branch_union() {
    // An IF used as a value (`RETURN IF …`, which lowers to a subquery)
    // types as the union of its branch values — not `unknown`.
    let mut workspace = Workspace::default();
    let query =
        workspace.add_virtual_source("query".into(), "RETURN IF $c { 1 } ELSE { 'x' };".into());

    let output = analyze_workspace(&workspace);

    assert_eq!(
        output.sources[&query].response_kind,
        Some(Kind::Either(vec![Kind::Int, Kind::String])),
    );
}

#[test]
fn none_guard_narrows_the_right_operand_for_arg_checks() {
    let mut workspace = Workspace::default();
    // `$value = NONE OR string::len($value) = 10`: inside the right
    // operand `$value` is non-none, so `string::len` sees `string`, not
    // `option<string>` — no argument-type finding (5002).
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE t SCHEMAFULL;\n\
         DEFINE FIELD code ON t TYPE option<string>\n\
             ASSERT $value = NONE OR string::len($value) = 10;\n\
         DEFINE FIELD mail ON t TYPE option<string>\n\
             ASSERT $value != NONE AND string::is_email($value);"
            .into(),
    );

    let output = analyze_workspace(&workspace);

    assert_eq!(codes(&output, 5002), 0, "{:?}", output.diagnostics);
}

#[test]
fn every_spelling_of_the_or_guard_narrows_the_right_operand() {
    // One fact, four spellings. The recognizer this replaced accepted
    // exactly one of them — `$x = NONE OR …`, written with `=`, on a bare
    // param — so the other three raised a 5002 on code the analyzer had
    // already proven safe. All four now go through the same guard IR.
    let spellings = [
        "$value IS NONE OR string::len($value) = 10",
        "!($value != NONE) OR string::len($value) = 10",
        "type::is_none($value) OR string::len($value) = 10",
        "$value = NONE OR $value = '' OR string::len($value) = 10",
    ];
    for assertion in spellings {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            format!(
                "DEFINE TABLE t SCHEMAFULL;\n\
                 DEFINE FIELD code ON t TYPE option<string> ASSERT {assertion};"
            ),
        );
        assert_eq!(
            codes(&analyze_workspace(&workspace), 5002),
            0,
            "`{assertion}`"
        );
    }
}

#[test]
fn an_unguarded_optional_argument_still_fails_the_arg_check() {
    let mut workspace = Workspace::default();
    // Without the `= NONE OR` guard the optional reaches `string::len`
    // as `option<string>`, which is a genuine 5002.
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE t SCHEMAFULL;\n\
         DEFINE FIELD code ON t TYPE option<string>\n\
             ASSERT string::len($value) = 10;"
            .into(),
    );

    let output = analyze_workspace(&workspace);

    assert_eq!(codes(&output, 5002), 1);
}

#[test]
fn analyze_workspace_infers_return_shape_from_let_variable() {
    let mut workspace = Workspace::default();
    let source =
        workspace.add_virtual_source("query".into(), "LET $age = 42;\nRETURN $age;".into());

    let output = analyze_workspace(&workspace);
    let return_statement = output.sources[&source]
        .statements
        .iter()
        .find(|statement| statement.kind == "return")
        .expect("return statement exists");

    assert_eq!(return_statement.response_kind, Some(Kind::Int));
}

#[test]
fn analyze_workspace_infers_let_variables_from_prior_let_variables() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "LET $age = 42;\nLET $next = $age + 1;\nRETURN $next;".into(),
    );

    let output = analyze_workspace(&workspace);
    let source_output = &output.sources[&source];
    let return_statement = source_output
        .statements
        .iter()
        .find(|statement| statement.kind == "return")
        .expect("return statement exists");

    assert!(source_output.inferred_params.is_empty());
    assert_eq!(return_statement.response_kind, Some(Kind::Int));
}

#[test]
fn analyze_workspace_uses_latest_let_shadow_for_return_shape() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "LET $x = 1;\nLET $x = 's';\nRETURN $x;".into(),
    );

    let output = analyze_workspace(&workspace);
    let source_output = &output.sources[&source];
    let return_statement = source_output
        .statements
        .iter()
        .find(|statement| statement.kind == "return")
        .expect("return statement exists");

    assert!(source_output.inferred_params.is_empty());
    assert_eq!(return_statement.response_kind, Some(Kind::String));
}

#[test]
fn analyze_workspace_preserves_prior_let_dependency_before_shadowing() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "LET $x = 1;\nLET $y = $x + 1;\nLET $x = 's';\nRETURN $y;".into(),
    );

    let output = analyze_workspace(&workspace);
    let source_output = &output.sources[&source];
    let return_statement = source_output
        .statements
        .iter()
        .find(|statement| statement.kind == "return")
        .expect("return statement exists");

    assert!(source_output.inferred_params.is_empty());
    assert_eq!(return_statement.response_kind, Some(Kind::Int));
}

#[test]
fn analyze_workspace_infers_if_else_return_shape_union() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        // A dynamic guard keeps both branches reachable (a constant guard
        // would fold to a single branch); this exercises the shape union.
        "IF $c { RETURN 1; } ELSE { RETURN 's'; };".into(),
    );

    let output = analyze_workspace(&workspace);
    let source_output = &output.sources[&source];
    let if_statement = source_output
        .statements
        .iter()
        .find(|statement| statement.kind == "if_else")
        .expect("if/else statement exists");

    assert_eq!(
        if_statement.response_kind,
        Some(Kind::Either(vec![Kind::Int, Kind::String,]))
    );
    assert_eq!(source_output.response_kind, if_statement.response_kind);
}

#[test]
fn analyze_workspace_collapses_matching_if_else_return_shapes() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "IF true { RETURN 1; } ELSE { RETURN 2; };".into(),
    );

    let output = analyze_workspace(&workspace);
    let if_statement = output.sources[&source]
        .statements
        .iter()
        .find(|statement| statement.kind == "if_else")
        .expect("if/else statement exists");

    assert_eq!(if_statement.response_kind, Some(Kind::Int));
}

#[test]
fn analyze_workspace_uses_prior_let_variables_in_if_else_return_shapes() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "LET $age = 42;\nIF true { RETURN $age; } ELSE { RETURN $age + 1; };".into(),
    );

    let output = analyze_workspace(&workspace);
    let source_output = &output.sources[&source];
    let if_statement = source_output
        .statements
        .iter()
        .find(|statement| statement.kind == "if_else")
        .expect("if/else statement exists");

    assert!(source_output.inferred_params.is_empty());
    assert_eq!(if_statement.response_kind, Some(Kind::Int));
}

#[test]
fn analyze_workspace_reports_non_bool_if_condition_literals() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "IF 1 { RETURN 1; } ELSE { RETURN 2; };\nIF 'yes' { RETURN 1; } ELSE { RETURN 2; };".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    assert!(messages.iter().any(|(code, message)| {
        code == "E2005" && message == "this IF condition is a `int`, not a `bool`"
    }));
    assert!(messages.iter().any(|(code, message)| {
        code == "E2005" && message == "this IF condition is a `string`, not a `bool`"
    }));
}

#[test]
fn analyze_workspace_allows_bool_and_dynamic_if_conditions() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "LET $flag = true;\nIF $flag { RETURN 1; } ELSE { RETURN 2; };\nIF $runtime { RETURN 1; } ELSE { RETURN 2; };".into(),
    );

    let output = analyze_workspace(&workspace);
    let source_output = &output.sources[&source];

    assert!(source_output
        .diagnostics
        .iter()
        .all(|finding| finding.code().to_string() != "E2005"));
    assert_eq!(source_output.inferred_params.len(), 1);
    assert_eq!(source_output.inferred_params[0].name, "runtime");
    assert_eq!(source_output.inferred_params[0].kind, None);
}

#[test]
fn analyze_workspace_reports_non_bool_if_condition_from_let() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "LET $flag = 1;\nIF $flag { RETURN 1; } ELSE { RETURN 2; };".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    assert!(messages.iter().any(|(code, message)| {
        code == "E2005" && message == "this IF condition is a `int`, not a `bool`"
    }));
}

#[test]
fn analyze_workspace_reports_non_bool_if_condition_from_branch_local_let() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "IF true { LET $flag = 1; IF $flag { RETURN 1; } ELSE { RETURN 2; }; } ELSE { RETURN 3; };"
            .into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    assert!(messages.iter().any(|(code, message)| {
        code == "E2005" && message == "this IF condition is a `int`, not a `bool`"
    }));
}

#[test]
fn analyze_workspace_keeps_if_branch_let_variables_local() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "IF true { LET $branch = 1; } ELSE { LET $branch = 2; };\nRETURN $branch;".into(),
    );

    let output = analyze_workspace(&workspace);
    let source_output = &output.sources[&source];
    let return_statement = source_output
        .statements
        .iter()
        .find(|statement| statement.kind == "return")
        .expect("return statement exists");

    assert_eq!(source_output.inferred_params.len(), 1);
    assert_eq!(source_output.inferred_params[0].name, "branch");
    assert_eq!(source_output.inferred_params[0].kind, None);
    assert!(matches!(return_statement.response_kind, Some(Kind::Any)));
}

#[test]
fn analyze_workspace_resolves_if_branch_local_let_returns() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        // A dynamic guard keeps both branches reachable (a constant guard
        // would fold to a single branch); this exercises the value union.
        "IF $c { LET $branch = 1; RETURN $branch; } ELSE { LET $branch = 's'; RETURN $branch; };"
            .into(),
    );

    let output = analyze_workspace(&workspace);
    let if_statement = output.sources[&source]
        .statements
        .iter()
        .find(|statement| statement.kind == "if_else")
        .expect("if statement exists");

    let Some(Kind::Either(variants)) = &if_statement.response_kind else {
        panic!(
            "expected IF branch either kind, got {:?}",
            if_statement.response_kind
        );
    };
    assert_eq!(variants.len(), 2);
    assert!(variants.contains(&Kind::Int));
    assert!(variants.contains(&Kind::String));
}

#[test]
fn analyze_workspace_outer_let_is_visible_inside_if_branch() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "LET $outer = 1;\nIF true { LET $branch = $outer + 1; RETURN $branch; } ELSE { RETURN $outer; };".into(),
    );

    let output = analyze_workspace(&workspace);
    let if_statement = output.sources[&source]
        .statements
        .iter()
        .find(|statement| statement.kind == "if_else")
        .expect("if statement exists");

    assert_eq!(if_statement.response_kind, Some(Kind::Int));
}
