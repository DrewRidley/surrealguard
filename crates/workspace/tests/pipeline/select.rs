//! `SELECT` contracts and response-shape inference: projections, clause
//! field validation, GROUP/aggregate contracts, `SELECT VALUE`, brace
//! selectors, and modifier facts.

use surrealdb_types::{Kind, KindLiteral};
use surrealguard_diagnostics::FindingCode;
use surrealguard_workspace::{analyze_query, analyze_workspace, render_kind, Workspace};

use crate::support::{assert_no_syntax_findings, codes};

#[test]
fn select_from_a_record_param_types_against_its_table() {
    // `SELECT … FROM ONLY $p` where `$p: record<T>` projects against T's
    // fields, not `Any`.
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE org SCHEMAFULL;\n\
         DEFINE TABLE unit SCHEMAFULL;\n\
         DEFINE FIELD org ON unit TYPE record<org>;\n\
         DEFINE FUNCTION fn::u($id: record<unit>) { RETURN (SELECT org FROM ONLY $id); };"
            .into(),
    );
    let query = workspace.add_virtual_source("query".into(), "RETURN fn::u($x);".into());

    let output = analyze_workspace(&workspace);
    let rendered = render_kind(
        output.sources[&query]
            .response_kind
            .as_ref()
            .expect("response kind"),
    );
    assert!(
        rendered.contains("org: record<org>"),
        "expected the projected field typed, got {rendered}"
    );
}

// ---- LIVE SELECT positions are SELECT positions ----

#[test]
fn live_select_projection_and_filter_fields_are_checked() {
    let schema = "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;";
    let mut workspace = Workspace::default();
    workspace.add_virtual_source("schema".into(), schema.into());
    workspace.add_virtual_source(
        "query".into(),
        "LIVE SELECT bogus FROM person WHERE nope = 1;".into(),
    );
    let output = analyze_workspace(&workspace);
    assert_eq!(codes(&output, 1002), 2, "{:?}", output.diagnostics);

    let mut workspace = Workspace::default();
    workspace.add_virtual_source("schema".into(), schema.into());
    workspace.add_virtual_source(
        "query".into(),
        "LIVE SELECT name FROM person WHERE name = 'a';\nLIVE SELECT DIFF FROM person;".into(),
    );
    let output = analyze_workspace(&workspace);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

// ---- typed operators: containment needs a collection operand (2004) ----

#[test]
fn containment_operators_need_an_operand_that_can_hold_members() {
    let schema = "DEFINE TABLE person SCHEMAFULL;\n\
                  DEFINE FIELD name ON person TYPE string;\n\
                  DEFINE FIELD age ON person TYPE int;\n\
                  DEFINE FIELD tags ON person TYPE array<string>;";
    let cases = [
        ("SELECT * FROM person WHERE age CONTAINS 1;", 1),
        ("SELECT * FROM person WHERE 1 IN age;", 1),
        ("SELECT * FROM person WHERE 1 INSIDE age;", 1),
        ("SELECT * FROM person WHERE tags CONTAINS 'x';", 0),
        ("SELECT * FROM person WHERE 'x' IN tags;", 0),
        ("SELECT * FROM person WHERE name CONTAINS 'x';", 0),
        ("SELECT * FROM person WHERE tags CONTAINSANY ['x'];", 0),
    ];
    for (query, expected) in cases {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source("schema".into(), schema.into());
        workspace.add_virtual_source("query".into(), query.into());
        let output = analyze_workspace(&workspace);
        assert_eq!(
            codes(&output, 2004),
            expected,
            "`{query}`: {:?}",
            output.diagnostics
        );
    }
}

// ---- 4013: GROUP BY field not in projections ----

#[test]
fn group_by_key_not_projected_fires_4013_once() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE sale;\nDEFINE FIELD region ON sale TYPE string;\nDEFINE FIELD amount ON sale TYPE int;".into(),
    );
    // `region` is grouped but never projected, so the grouped rows carry no
    // region label — a footgun SurrealDB runs silently.
    workspace.add_virtual_source(
        "query".into(),
        "SELECT math::sum(amount) AS total FROM sale GROUP BY region;".into(),
    );
    let output = analyze_workspace(&workspace);
    assert_eq!(codes(&output, 4013), 1, "{:?}", output.diagnostics);
}

#[test]
fn group_by_key_projected_stays_silent_for_4013() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE sale;\nDEFINE FIELD region ON sale TYPE string;\nDEFINE FIELD amount ON sale TYPE int;".into(),
    );
    // `region` is projected (bare) and `year` is projected as an alias that
    // the GROUP BY names — both forms cover the grouping key.
    workspace.add_virtual_source(
        "query".into(),
        "SELECT region, math::sum(amount) AS total FROM sale GROUP BY region;".into(),
    );
    let output = analyze_workspace(&workspace);
    assert_eq!(codes(&output, 4013), 0, "{:?}", output.diagnostics);
}

// ---- 4025: wildcard projection under a GROUP clause ----

#[test]
fn wildcard_under_group_fires_4025_once_per_wildcard() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE sale;\nDEFINE FIELD region ON sale TYPE string;".into(),
    );
    // SurrealDB 3.0.5 rejects both forms outright; 2.x drops the `*`.
    workspace.add_virtual_source(
        "query".into(),
        "SELECT * FROM sale GROUP BY region;\nSELECT *, count() FROM sale GROUP ALL;".into(),
    );
    let output = analyze_workspace(&workspace);
    assert_eq!(codes(&output, 4025), 2, "{:?}", output.diagnostics);
    assert!(
        output
            .diagnostics
            .iter()
            .filter(|finding| finding.code().number() == 4025)
            .all(|finding| finding.severity() == surrealguard_diagnostics::Severity::Error),
        "4025 is an error: the engine rejects the query"
    );
}

#[test]
fn explicit_group_projections_stay_silent_for_4025() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE TABLE sale;\nDEFINE FIELD region ON sale TYPE string;\nDEFINE FIELD amount ON sale TYPE int;".into(),
    );
    workspace.add_virtual_source(
        "query".into(),
        "SELECT region, math::sum(amount) AS total FROM sale GROUP BY region;\nSELECT count() FROM sale GROUP ALL;\nSELECT * FROM sale;".into(),
    );
    let output = analyze_workspace(&workspace);
    assert_eq!(codes(&output, 4025), 0, "{:?}", output.diagnostics);
}

#[test]
fn bare_count_resolves_without_unknown_function() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source("schema".into(), "DEFINE TABLE person SCHEMAFULL;".into());

    // Bare `count()` must resolve to the builtin (no spurious 5001), and a
    // grouped count is a real aggregate (no 4023 either).
    let output = analyze_query(&mut workspace, "SELECT count() AS n FROM person GROUP ALL;");

    assert_eq!(
        output
            .diagnostics
            .iter()
            .filter(|f| f.code().number() == 5001)
            .count(),
        0,
        "unexpected 5001: {:?}",
        output.diagnostics
    );
    assert_eq!(
        output
            .diagnostics
            .iter()
            .filter(|f| f.code().number() == 4023)
            .count(),
        0,
    );
}

#[test]
fn ungrouped_bare_count_fires_4023_not_5001() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source("schema".into(), "DEFINE TABLE person SCHEMAFULL;".into());

    let output = analyze_query(&mut workspace, "SELECT count() AS n FROM person;");

    assert_eq!(
        output
            .diagnostics
            .iter()
            .filter(|f| f.code().number() == 5001)
            .count(),
        0,
        "unexpected 5001: {:?}",
        output.diagnostics
    );
    assert_eq!(
        output
            .diagnostics
            .iter()
            .filter(|f| f.code().number() == 4023)
            .count(),
        1,
        "expected 4023: {:?}",
        output.diagnostics
    );
}

/// The consumer decides. `array::is_empty(SELECT count() …)` is an
/// existence test, and the two spellings are **not** interchangeable
/// there — engine-verified on 3.0.5:
///
/// ```text
/// array::is_empty(SELECT count() FROM ea WHERE <no match>)            -> true
/// array::is_empty(SELECT count() FROM ea WHERE <no match> GROUP ALL)  -> false
/// ```
///
/// So 4023's advice ("add GROUP ALL for a total") inverts this guard
/// rather than fixing it. The corpus shape it was firing on is
/// `fn::entity::permissible`, the workshop database's central ACL check.
#[test]
fn ungrouped_count_consumed_by_is_empty_does_not_fire_4023() {
    for query in [
        "RETURN array::is_empty(SELECT count() FROM person WHERE age > 18) = false;",
        "RETURN array::is_empty(SELECT count() FROM person);",
        "RETURN array::len(SELECT count() FROM person) > 0;",
        "RETURN count(SELECT count() FROM person);",
        "IF (SELECT count() FROM person) { RETURN 1; };",
    ] {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD age ON person TYPE int;".into(),
        );
        let output = analyze_query(&mut workspace, query);
        assert!(
            !output.diagnostics.iter().any(|f| f.code().number() == 4023),
            "4023 must not fire in a cardinality position: {query}\n{:?}",
            output.diagnostics
        );
    }
}

/// …and it must keep firing where it is right. `SELECT count()` read as a
/// *number* is the real footgun: N rows of `{count: 1}`, never one row of
/// `{count: N}`, so the guard below never fires.
#[test]
fn ungrouped_count_compared_to_a_number_still_fires_4023() {
    for query in [
        "LET $n = (SELECT count() FROM person); IF $n = 0 { THROW 'none'; };",
        "RETURN (SELECT count() FROM person);",
        // The marker names one position: a nested SELECT inside the
        // consumed one is read as a number and still reports.
        "RETURN array::is_empty(SELECT count() FROM person \
         WHERE age = (SELECT count() FROM person));",
    ] {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD age ON person TYPE int;".into(),
        );
        let output = analyze_query(&mut workspace, query);
        assert!(
            output.diagnostics.iter().any(|f| f.code().number() == 4023),
            "expected 4023 for {query}: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn analyze_workspace_infers_select_comparison_and_boolean_projection_shapes() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD active ON person TYPE bool;\nSELECT age > 18 AS adult, active = true AS matches_active, true AND active AS visible FROM person;".into(),
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

    assert_eq!(fields["adult"], Kind::Bool);
    assert_eq!(fields["matches_active"], Kind::Bool);
    assert_eq!(fields["visible"], Kind::Bool);
}

#[test]
fn analyze_workspace_allows_known_select_projection_fields() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD profile.email ON person TYPE string;\nSELECT name, profile.email FROM person;".into(),
    );

    let output = analyze_workspace(&workspace);

    assert!(output
        .diagnostics
        .iter()
        .all(|finding| finding.code() != FindingCode::schema(1002)));
}

#[test]
fn analyze_workspace_reports_unknown_select_projection_fields() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT nickname, profile.phone FROM person;".into(),
    );

    let output = analyze_workspace(&workspace);
    let unknown_fields: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1002))
        .collect();

    assert_eq!(unknown_fields.len(), 2);
    assert_eq!(
        unknown_fields[0].message(),
        "`person` has no field `nickname`"
    );
    assert_eq!(unknown_fields[0].span().source(), &source);
    assert_eq!(unknown_fields[0].span().range().start(), 69);
    assert_eq!(unknown_fields[0].span().range().end(), 77);
    assert_eq!(
        unknown_fields[1].message(),
        "`person` has no field `profile.phone`"
    );
    assert_eq!(unknown_fields[1].span().range().start(), 79);
    assert_eq!(unknown_fields[1].span().range().end(), 92);
    // The two unknown fields (1002) plus the whole-table read lint (7014,
    // allow-by-default): this SELECT has no WHERE/LIMIT.
    assert_eq!(output.sources[&source].diagnostics.len(), 3);
}

#[test]
fn analyze_workspace_reports_unknown_select_order_fields() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT * FROM person ORDER BY missing;".into(),
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
fn analyze_workspace_reports_unknown_select_group_and_split_fields() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT name FROM person GROUP BY missing_group SPLIT missing_split;".into(),
    );

    let output = analyze_workspace(&workspace);
    let unknown_fields: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .filter(|finding| matches!(finding.code(), code if code == FindingCode::schema(1002) || code == FindingCode::schema(1002)))
        .map(|finding| finding.message().to_string())
        .collect();
    let mut unknown_fields = unknown_fields;
    unknown_fields.sort();

    assert_eq!(
        unknown_fields,
        vec![
            "`person` has no field `missing_group`".to_string(),
            "`person` has no field `missing_split`".to_string(),
        ]
    );
}

#[test]
fn analyze_workspace_reports_unknown_select_where_fields() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT * FROM person WHERE missing = true AND name = 'Ada';".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1002))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(messages, vec!["`person` has no field `missing`"]);
}

#[test]
fn analyze_workspace_validates_aliased_select_projection_source_field() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT nickname AS display_name FROM person;".into(),
    );

    let output = analyze_workspace(&workspace);
    let unknown_fields: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| finding.code() == FindingCode::schema(1002))
        .collect();

    assert_eq!(unknown_fields.len(), 1);
    assert_eq!(
        unknown_fields[0].message(),
        "`person` has no field `nickname`"
    );
}

#[test]
fn analyze_workspace_skips_wildcard_select_projection_field_validation() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nSELECT * FROM person;".into(),
    );

    let output = analyze_workspace(&workspace);

    assert!(output
        .diagnostics
        .iter()
        .all(|finding| finding.code() != FindingCode::schema(1002)));
}

#[test]
fn analyze_workspace_validates_and_shapes_parent_object_paths() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD profile.name ON person TYPE string;\nSELECT profile FROM person WHERE profile = $profile;".into(),
    );

    let output = analyze_workspace(&workspace);
    assert!(output
        .diagnostics
        .iter()
        .all(|finding| finding.code() != FindingCode::schema(1002)));
    let params = &output.sources[&source].inferred_params;
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, "profile");
    // The comparison against the declared object field constrains the
    // parameter to that field's shape.
    assert_eq!(
        params[0].kind,
        Some(Kind::Literal(surrealdb_types::KindLiteral::Object(
            std::collections::BTreeMap::from([("name".to_string(), Kind::String)])
        )))
    );

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
    let Kind::Literal(KindLiteral::Object(profile_fields)) = &fields["profile"] else {
        panic!(
            "expected nested object literal, got {:?}",
            fields["profile"]
        );
    };
    assert_eq!(profile_fields["name"], Kind::String);
}

#[test]
fn analyze_workspace_infers_row_brace_selector_without_alias_shape() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD profile.name ON person TYPE string;\nDEFINE FIELD profile.age ON person TYPE int;\nDEFINE FIELD profile.secret ON person TYPE string;\nSELECT profile.{name, age} FROM person;".into(),
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
    let Kind::Literal(KindLiteral::Object(profile_fields)) = &fields["profile"] else {
        panic!(
            "expected nested object literal, got {:?}",
            fields["profile"]
        );
    };

    assert_eq!(
        profile_fields
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["age", "name"]
    );
    assert_eq!(profile_fields["name"], Kind::String);
    assert_eq!(profile_fields["age"], Kind::Int);
}

#[test]
fn analyze_workspace_infers_row_brace_selector_alias_shape() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD profile.name ON person TYPE string;\nDEFINE FIELD profile.age ON person TYPE int;\nSELECT profile.{name, age} AS public_profile FROM person;".into(),
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
    let Kind::Literal(KindLiteral::Object(profile_fields)) = &fields["public_profile"] else {
        panic!(
            "expected nested object literal, got {:?}",
            fields["public_profile"]
        );
    };

    assert_eq!(profile_fields["name"], Kind::String);
    assert_eq!(profile_fields["age"], Kind::Int);
}

#[test]
fn analyze_workspace_infers_select_value_expression_response_shape() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nSELECT VALUE age + 1 FROM person;".into(),
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
    assert_eq!(element.as_ref(), &Kind::Int);
}

#[test]
fn analyze_workspace_infers_select_value_function_response_shape() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT VALUE string::lowercase(name) FROM person;".into(),
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
    assert_eq!(element.as_ref(), &Kind::String);
}

#[test]
fn analyze_workspace_validates_omit_and_fetch_field_paths() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT * OMIT password FROM person FETCH friend;".into(),
    );

    let output = analyze_workspace(&workspace);
    let unknown_fields: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|finding| matches!(finding.code(), code if code == FindingCode::schema(1002) || code == FindingCode::schema(1002)))
        .map(|finding| finding.message().to_string())
        .collect();

    assert_eq!(
        unknown_fields,
        vec![
            "`person` has no field `password`",
            "`person` has no field `friend`",
        ]
    );
}

#[test]
fn analyze_workspace_exposes_row_preserving_select_modifier_facts() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT name FROM person WHERE name = 'Ada' ORDER BY name LIMIT 5 START 2 TIMEOUT 1s PARALLEL;".into(),
    );

    let output = analyze_workspace(&workspace);
    let select = output.sources[&source]
        .statements
        .iter()
        .find(|statement| statement.kind == "select")
        .expect("select statement exists");

    let modifiers: Vec<_> = select
        .select_modifiers
        .iter()
        .map(|modifier| {
            (
                modifier.kind.as_str(),
                modifier.row_preserving,
                modifier.max_len,
            )
        })
        .collect();

    assert_eq!(
        modifiers,
        vec![
            ("where", true, None),
            ("order", true, None),
            ("limit", true, Some(5)),
            ("start", true, None),
            ("timeout", true, None),
            ("parallel", true, None),
        ]
    );
}

#[test]
fn analyze_workspace_exposes_group_by_collapsing_select_modifier_facts() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nSELECT name FROM person GROUP BY name;".into(),
    );

    let output = analyze_workspace(&workspace);
    assert_no_syntax_findings(&output.sources[&source].diagnostics);
    let select = output.sources[&source]
        .statements
        .iter()
        .find(|statement| statement.kind == "select")
        .expect("select statement exists");

    // GROUP BY aggregates rows, so the fact is not row-preserving.
    let group = select
        .select_modifiers
        .iter()
        .find(|modifier| modifier.kind == "group")
        .expect("group modifier fact exists");
    assert!(!group.row_preserving);
}
