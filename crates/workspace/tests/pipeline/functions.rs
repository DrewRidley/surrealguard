//! Function calls: builtin and `fn::` signatures, argument contracts,
//! cross-source UDF return resolution, and the recursion (5009) carve-outs.

use surrealdb_types::{Kind, KindLiteral};
use surrealql_analyzer_workspace::{analyze_workspace, Workspace};

use crate::support::{assert_no_syntax_findings, codes};

#[test]
fn cross_source_udf_call_resolves_a_table_bearing_body_return() {
    // A function whose body reads a table, defined in one source and called
    // from another: the call must resolve the real return type, not `Any`.
    // (The cross-source catalog import used to infer the body against an
    // empty schema, degrading table-bearing bodies to `Any`.)
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE unit SCHEMAFULL;\n\
         DEFINE FIELD label ON unit TYPE string;\n\
         DEFINE FUNCTION fn::pick() { RETURN (SELECT VALUE label FROM ONLY unit); };"
            .into(),
    );
    let query = workspace.add_virtual_source("query".into(), "RETURN fn::pick();".into());

    let output = analyze_workspace(&workspace);

    // `option<string>`, not `string`: the body's `FROM ONLY unit` is
    // optional (an empty `unit` table makes it NONE). The point of the
    // test is that the call resolves the body's *table-bearing* type at
    // all rather than degrading to `Any`.
    assert_eq!(
        output.sources[&query].response_kind,
        Some(Kind::either(vec![Kind::None, Kind::String])),
        "cross-source UDF call must resolve its table-bearing body return"
    );
}

#[test]
fn function_arg_mismatch_renders_optional_kinds_idiomatically() {
    let mut workspace = Workspace::default();
    // Passing an `int` where a param declared `option<string>` is expected
    // trips the 5002 arg-type contract. The declared kind must render in
    // the idiomatic compact form (`option<string>`), never the raw union
    // (`none | string`).
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE FUNCTION fn::greet($name: option<string>) { RETURN $name; };\n\
         RETURN fn::greet(1);"
            .into(),
    );

    let output = analyze_workspace(&workspace);
    let finding = output.sources[&source]
        .diagnostics
        .iter()
        .find(|finding| finding.code().number() == 5002)
        .expect("a 5002 argument-type finding");
    let message = finding.message().to_string();

    assert!(
        message.contains("declared `option<string>`"),
        "expected compact optional rendering, got: {message}"
    );
    assert!(
        message.contains("is a `1`"),
        "expected the passed kind, got: {message}"
    );
    assert!(!message.contains("none |"), "raw union leaked: {message}");
    // The arg-mismatch note points back at the DEFINE FUNCTION.
    assert!(
        finding
            .related()
            .iter()
            .any(|note| note.message == "`fn::greet` is defined here"),
        "expected a related note at the function definition, got: {:?}",
        finding.related()
    );
}

#[test]
fn analyze_workspace_infers_extended_verified_function_projection_shapes_and_params() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD title ON person TYPE string;\nDEFINE FIELD tags ON person TYPE array;\nSELECT string::lowercase(name) AS lower, string::uppercase(name) AS upper, string::contains(name, 'a') AS has_needle, string::starts_with(name, 'a') AS starts, string::ends_with(name, 'z') AS ends, array::is_empty(tags) AS no_items FROM person;".into(),
    );

    let output = analyze_workspace(&workspace);
    let select = output.sources[&source]
        .statements
        .iter()
        .find(|statement| statement.kind == "select")
        .expect("select statement exists");

    let Some(Kind::Array(element, _)) = &select.response_kind else {
        panic!("expected array kind, got {:?}", select.response_kind);
    };
    let Kind::Literal(KindLiteral::Object(fields)) = element.as_ref() else {
        panic!("expected object literal element, got {element:?}");
    };
    assert_eq!(fields["lower"], Kind::String);
    assert_eq!(fields["upper"], Kind::String);
    assert_eq!(fields["has_needle"], Kind::Bool);
    assert_eq!(fields["starts"], Kind::Bool);
    assert_eq!(fields["ends"], Kind::Bool);
    assert_eq!(fields["no_items"], Kind::Bool);
}

#[test]
fn analyze_workspace_reports_function_argument_mismatch_from_branch_local_let() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nIF true { LET $age = 42; SELECT string::len($age) FROM person; };"
            .into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    assert!(messages.iter().any(|(code, message)| {
        code == "E5002"
            && message == "argument 1 to `string::len` is a `int`, but `string` is required"
    }));
}

#[test]
fn analyze_workspace_reports_function_mismatch_in_mutation_return_expression() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nLET $age = 42;\nUPDATE person SET age = $age RETURN string::len($age) AS bad_len;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    assert!(messages.iter().any(|(code, message)| {
        code == "E5002"
            && message == "argument 1 to `string::len` is a `int`, but `string` is required"
    }));
}

#[test]
fn analyze_workspace_reports_function_misuse_in_where_and_set_positions() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nSELECT * FROM person WHERE string::len(age) > 0;\nUPDATE person SET age = math::abs('x');".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    // WHERE conditions and SET values are walked like any other
    // expression position.
    assert!(messages.iter().any(|(code, message)| {
        code == "E5002"
            && message == "argument 1 to `string::len` is a `int`, but `string` is required"
    }));
    assert!(messages.iter().any(|(code, message)| {
        code == "E5002"
            && message == "argument 1 to `math::abs` is a `string`, but a number is required"
    }));
}

#[test]
fn analyze_workspace_reports_unknown_custom_functions() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE FUNCTION fn::greet($name: string) -> string { RETURN 'hi'; };\nRETURN fn::greet('a');\nRETURN fn::gret('a');".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    assert!(messages.iter().any(|(code, message)| {
        code == "E5001" && message == "`fn::gret` is not a defined function"
    }));
    // The defined function produces no finding.
    assert!(!messages
        .iter()
        .any(|(_, message)| message.contains("fn::greet")));
}

#[test]
fn analyze_workspace_reports_function_arity_and_argument_kind_mismatches() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nSELECT string::len(), string::len(age), array::len(age), unknown::fn(age) FROM person;".into(),
    );

    let output = analyze_workspace(&workspace);
    let source_output = &output.sources[&source];
    let messages: Vec<_> = source_output
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    assert!(messages.iter().any(|(code, message)| {
        code == "E5002" && message == "`string::len` takes 1 argument, but this call passes 0"
    }));
    assert!(messages.iter().any(|(code, message)| {
        code == "E5002"
            && message == "argument 1 to `string::len` is a `int`, but `string` is required"
    }));
    assert!(messages.iter().any(|(code, message)| {
        code == "E5002"
            && message == "argument 1 to `array::len` is a `int`, but an array is required"
    }));
    assert!(messages.iter().any(|(code, message)| {
        code == "E5001" && message == "`unknown::fn` is not a known function"
    }));
}

#[test]
fn analyze_workspace_either_of_arrays_satisfies_array_argument() {
    // `$rows ?? []` infers `array<...> | array<any, 0>` — a union whose
    // every variant is an array, so it satisfies `array::concat`'s array
    // parameter. No 5002.
    let mut workspace = Workspace::default();
    let _source = workspace.add_virtual_source(
        "query".into(),
        concat!(
            "DEFINE TABLE org SCHEMAFULL;\n",
            "DEFINE FUNCTION fn::principals() {\n",
            "  LET $orgs = SELECT VALUE id FROM org;\n",
            "  RETURN array::concat([], $orgs ?? []);\n",
            "};\n",
        )
        .into(),
    );

    let output = analyze_workspace(&workspace);
    assert_eq!(
        codes(&output, 5002),
        0,
        "an array-or-empty-array union is an array: {:?}",
        output.diagnostics
    );
}

#[test]
fn analyze_workspace_accepts_a_diamond_function_call_graph() {
    let mut workspace = Workspace::default();
    // fn::a fans out to b and c, both call d, d calls nothing. A DAG,
    // not a cycle, so no non-termination (5009) finding.
    let source = workspace.add_virtual_source(
        "query".into(),
        concat!(
            "DEFINE FUNCTION fn::a() { RETURN fn::b() + fn::c(); };\n",
            "DEFINE FUNCTION fn::b() { RETURN fn::d(); };\n",
            "DEFINE FUNCTION fn::c() { RETURN fn::d(); };\n",
            "DEFINE FUNCTION fn::d() { RETURN 1; };\n",
        )
        .into(),
    );

    let output = analyze_workspace(&workspace);
    assert_no_syntax_findings(&output.sources[&source].diagnostics);
    assert!(
        !output
            .diagnostics
            .iter()
            .any(|finding| finding.code().to_string().ends_with("5009")),
        "diamond call graph must not report non-termination, have: {:?}",
        output
            .diagnostics
            .iter()
            .map(|f| (f.code().to_string(), f.message().to_string()))
            .collect::<Vec<_>>()
    );
}

/// 5009's guardedness carve-out reads the lowered body, not the text of it.
///
/// The predicate used to be `has_keyword(body_text, "if")` over the raw
/// lowercased source, so a self-recursive function whose only `if` was
/// inside a string literal or a comment was classified as branching and
/// spared. Both bodies below recurse unconditionally and both must report.
#[test]
fn the_word_if_in_a_string_is_not_a_branch() {
    for body in [
        "RETURN 'if you see this' + fn::x();",
        "-- for now\n\tRETURN fn::x();",
    ] {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            format!("DEFINE FUNCTION fn::x() {{ {body} }};"),
        );
        let output = analyze_workspace(&workspace);
        assert!(
            output
                .diagnostics
                .iter()
                .any(|finding| finding.code().number() == 5009),
            "expected 5009 for a body that only mentions the word: {body:?}\n{:?}",
            output.diagnostics
        );
    }
}

/// …and a real branch still spares it, which is the load-bearing half
/// (`docs/plans/2026-07-25-analyzer-gap-backlog.md` DX-12).
#[test]
fn a_real_branch_still_spares_a_self_call() {
    for body in [
        "IF $n = 0 { RETURN 0; }; RETURN fn::x($n - 1);",
        "FOR $i IN [1] { RETURN fn::x($n); }; RETURN 0;",
        "RETURN IF $n = 0 { 0 } ELSE { fn::x($n - 1) };",
        "LET $r = IF $n = 0 { 0 } ELSE { fn::x($n - 1) }; RETURN $r;",
    ] {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            format!("DEFINE FUNCTION fn::x($n: int) {{ {body} }};"),
        );
        let output = analyze_workspace(&workspace);
        assert!(
            !output
                .diagnostics
                .iter()
                .any(|finding| finding.code().number() == 5009),
            "a branch can route around the self-call: {body:?}\n{:?}",
            output.diagnostics
        );
    }
}
