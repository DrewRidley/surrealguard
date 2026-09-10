//! Host parameters and `LET` bindings: which names are host params,
//! engine-bound names, inferred kinds and constraints, `DEFINE PARAM`
//! defaults, and the LET lints.

use surrealdb_types::Kind;
use surrealguard_workspace::analysis::ValueDomain;
use surrealguard_workspace::{analyze_workspace, Workspace};

use crate::support::codes;

// ---- 6002: LET shadows a DEFINE PARAM with a different kind ----

#[test]
fn let_shadows_define_param_with_incompatible_kind_fires_6002_once() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE PARAM $min_age VALUE 18;\nLET $min_age = 'old';\nRETURN $min_age;".into(),
    );
    let output = analyze_workspace(&workspace);
    assert_eq!(codes(&output, 6002), 1, "{:?}", output.diagnostics);
}

#[test]
fn let_shadows_define_param_with_compatible_kind_stays_silent_for_6002() {
    let mut workspace = Workspace::default();
    // Same int kind — a benign rebind, no clash.
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE PARAM $min_age VALUE 18;\nLET $min_age = 21;\nRETURN $min_age;".into(),
    );
    let output = analyze_workspace(&workspace);
    assert_eq!(codes(&output, 6002), 0, "{:?}", output.diagnostics);
}

// ---- 7001: unused LET binding (opt-in; default allow) ----

#[test]
fn unused_let_fires_7001_once() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source("query".into(), "LET $unused = 1;\nRETURN 5;".into());
    let output = analyze_workspace(&workspace);
    assert_eq!(codes(&output, 7001), 1, "{:?}", output.diagnostics);
}

#[test]
fn used_let_stays_silent_for_7001() {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source("query".into(), "LET $used = 1;\nRETURN $used;".into());
    let output = analyze_workspace(&workspace);
    assert_eq!(codes(&output, 7001), 0, "{:?}", output.diagnostics);
}

#[test]
fn let_used_only_in_nested_block_stays_silent_for_7001() {
    let mut workspace = Workspace::default();
    // The reference lives in a nested block that follows the LET — the
    // textual scan of the later sibling must find it.
    workspace.add_virtual_source(
        "query".into(),
        "LET $used = 1;\nIF true { RETURN $used; };".into(),
    );
    let output = analyze_workspace(&workspace);
    assert_eq!(codes(&output, 7001), 0, "{:?}", output.diagnostics);
}

#[test]
fn analyze_workspace_collects_query_parameters_by_name_with_spans() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nSELECT * FROM person WHERE id = $id OR owner = $id AND team = $team;".into(),
    );

    let output = analyze_workspace(&workspace);
    let params = &output.sources[&source].inferred_params;

    assert_eq!(params.len(), 2);
    assert_eq!(params[0].name, "id");
    assert_eq!(params[0].kind, None);
    assert!(params[0].required);
    assert_eq!(params[0].spans.len(), 2);
    assert_eq!(params[0].spans[0].range().start(), 53);
    assert_eq!(params[0].spans[0].range().end(), 56);
    assert_eq!(params[0].spans[1].range().start(), 68);
    assert_eq!(params[0].spans[1].range().end(), 71);
    assert_eq!(params[1].name, "team");
    assert_eq!(params[1].spans.len(), 1);
    assert_eq!(params[1].spans[0].range().start(), 83);
    assert_eq!(params[1].spans[0].range().end(), 88);
}

#[test]
fn analyze_workspace_excludes_let_variables_from_query_params() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "LET $age = 42;\nRETURN $age;\nSELECT * FROM person WHERE name = $name;".into(),
    );

    let output = analyze_workspace(&workspace);
    let params = &output.sources[&source].inferred_params;

    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, "name");
}

#[test]
fn analyze_workspace_seeds_auth_as_a_bound_fact_not_a_host_param() {
    // `$auth` is engine-supplied (the authenticated record, or NONE), never
    // host-supplied. A query comparing against it must not list it among
    // the inferred params — only the genuine host param `$name` remains.
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE post;\nSELECT * FROM post WHERE owner = $auth AND title = $name;".into(),
    );

    let output = analyze_workspace(&workspace);
    let names: Vec<_> = output.sources[&source]
        .inferred_params
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(names, vec!["name"], "`$auth` is engine-supplied");
}

#[test]
fn analyze_workspace_seeds_all_session_params_not_as_host_params() {
    // `$session`/`$access`/`$token`/`$scope` are engine-supplied too, so a
    // query that only references those has no host params.
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE post;\nSELECT * FROM post WHERE a = $session AND b = $access AND c = $token AND d = $scope;"
            .into(),
    );

    let output = analyze_workspace(&workspace);
    assert!(
        output.sources[&source].inferred_params.is_empty(),
        "session params are engine-supplied: {:?}",
        output.sources[&source].inferred_params
    );
}

#[test]
fn analyze_workspace_auth_member_access_does_not_fire_1002() {
    // `$auth` seeds as an open `option<record>`; member access degrades to
    // partial/Any (`field_of_kind` returns None on an empty-Record target),
    // so `$auth.id` / `$auth.whatever` never raise a false 1002.
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE post;\nSELECT * FROM post WHERE owner = $auth.id AND x = $auth.whatever;"
            .into(),
    );

    let output = analyze_workspace(&workspace);
    assert_eq!(codes(&output, 1002), 0, "{:?}", output.diagnostics);
}

#[test]
fn analyze_workspace_treats_forward_let_use_as_external_param() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "LET $y = $x + 1;\nLET $x = 1;\nRETURN $y;".into(),
    );

    let output = analyze_workspace(&workspace);
    let params = &output.sources[&source].inferred_params;

    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, "x");
}

#[test]
fn analyze_workspace_infers_param_kinds_from_function_signatures() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nSELECT string::len($name) AS name_len, array::len($tags) AS tag_count, count() AS total FROM person;".into(),
    );

    let output = analyze_workspace(&workspace);
    let params = &output.sources[&source].inferred_params;

    assert_eq!(params.len(), 2);
    assert_eq!(params[0].name, "name");
    assert_eq!(params[0].kind, Some(Kind::String));
    assert_eq!(params[1].name, "tags");
    assert_eq!(params[1].kind, Some(Kind::Array(Box::new(Kind::Any), None)));
}

#[test]
fn analyze_workspace_skips_function_param_inference_for_unknown_and_wrong_arity_calls() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nSELECT unknown::fn($value), string::len($first, $second), count($bad) FROM person;".into(),
    );

    let output = analyze_workspace(&workspace);
    let params = &output.sources[&source].inferred_params;

    // `$value` is context-bound (6005 territory), never a host param;
    // the other three stay unknown because their call sites teach
    // nothing (unknown function, wrong arity, count(any)).
    let names: Vec<_> = params.iter().map(|param| param.name.as_str()).collect();
    assert_eq!(names, vec!["bad", "first", "second"]);
    assert!(params.iter().all(|param| param.kind.is_none()));
}

/// One table, four disagreements it used to have.
///
/// Every name SurrealDB binds itself is protected from `LET` (6007) and is
/// never a host parameter. A *document* name used where no construct binds
/// one is 6005; a *positional* one (`$parent`) is bound by a nesting this
/// analyzer does not model, so it is neither a finding nor a host param.
#[test]
fn engine_bound_params_are_never_host_params() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "RETURN $this; RETURN $parent; RETURN $self; RETURN $scope; \
         RETURN $value; RETURN $real;"
            .into(),
    );

    let output = analyze_workspace(&workspace);
    let names: Vec<_> = output.sources[&source]
        .inferred_params
        .iter()
        .map(|param| param.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["real"],
        "only a name the engine does not bind is a host parameter"
    );

    let codes: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .map(|finding| finding.code().number())
        .collect();
    // `$this`, `$self` and `$value` are document params used outside any
    // document — the same finding `$value` alone used to get. `$parent`
    // and `$scope` are not: nothing here proves them unbound.
    assert_eq!(
        codes.iter().filter(|code| **code == 6005).count(),
        3,
        "expected 6005 for $this/$self/$value: {:?}",
        output.sources[&source].diagnostics
    );
}

#[test]
fn every_engine_bound_param_is_protected_from_let() {
    for name in [
        "auth", "session", "token", "access", "scope", "this", "self", "parent", "event", "value",
        "before", "after", "input",
    ] {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source("query".into(), format!("LET ${name} = 1;"));
        let output = analyze_workspace(&workspace);
        assert!(
            output.sources[&source]
                .diagnostics
                .iter()
                .any(|finding| finding.code().number() == 6007),
            "expected 6007 for `${name}`: {:?}",
            output.sources[&source].diagnostics
        );
    }
}

#[test]
fn analyze_workspace_infers_param_kind_from_select_where_field_comparison() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT * FROM person WHERE name = $name;".into(),
    );

    let output = analyze_workspace(&workspace);
    let params = &output.sources[&source].inferred_params;

    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, "name");
    assert_eq!(params[0].kind, Some(Kind::String));
}

#[test]
fn analyze_workspace_infers_param_kinds_from_select_where_comparison_operators() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD name ON person TYPE string;\nSELECT * FROM person WHERE age > $min_age AND $max_age >= age AND name != $excluded_name;".into(),
    );

    let output = analyze_workspace(&workspace);
    let params = &output.sources[&source].inferred_params;
    let param_kinds: Vec<_> = params
        .iter()
        .map(|param| (param.name.as_str(), param.kind.clone()))
        .collect();

    assert_eq!(
        param_kinds,
        vec![
            ("excluded_name", Some(Kind::String)),
            ("max_age", Some(Kind::Int)),
            ("min_age", Some(Kind::Int)),
        ]
    );
}

#[test]
fn analyze_workspace_infers_param_kinds_from_let_env_predicates() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD strength ON likes TYPE float;\nLET $min_age = 21;\nLET $edge_strength = 0.5;\nSELECT * FROM person WHERE $min_age <= $age_param;\nSELECT * FROM person->(likes WHERE $edge_strength >= $min_strength)->post;".into(),
    );

    let output = analyze_workspace(&workspace);
    let params = &output.sources[&source].inferred_params;
    let param_kinds: Vec<_> = params
        .iter()
        .map(|param| (param.name.as_str(), param.kind.clone()))
        .collect();

    assert_eq!(
        param_kinds,
        vec![
            ("age_param", Some(Kind::Int)),
            ("min_strength", Some(Kind::Float)),
        ]
    );
}

#[test]
fn analyze_workspace_infers_param_kinds_from_graph_local_predicates() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD created_at ON likes TYPE datetime;\nDEFINE FIELD strength ON likes TYPE float;\nSELECT * FROM person->(likes WHERE created_at > $since AND strength >= $min_strength)->post;\nSELECT * FROM person->likes[WHERE created_at <= $before]->post;".into(),
    );

    let output = analyze_workspace(&workspace);
    let params = &output.sources[&source].inferred_params;
    let param_kinds: Vec<_> = params
        .iter()
        .map(|param| (param.name.as_str(), param.kind.clone()))
        .collect();

    assert_eq!(
        param_kinds,
        vec![
            ("before", Some(Kind::Datetime)),
            ("min_strength", Some(Kind::Float)),
            ("since", Some(Kind::Datetime)),
        ]
    );
}

#[test]
fn analyze_workspace_infers_param_kinds_from_mutation_where_comparisons() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD name ON person TYPE string;\nUPDATE person SET name = 'Ada' WHERE age > $min_age AND $max_age >= age;\nUPSERT person SET name = 'Ada' WHERE name != $excluded_name;\nDELETE person WHERE age <= $delete_before;".into(),
    );

    let output = analyze_workspace(&workspace);
    let params = &output.sources[&source].inferred_params;
    let param_kinds: Vec<_> = params
        .iter()
        .map(|param| (param.name.as_str(), param.kind.clone()))
        .collect();

    assert_eq!(
        param_kinds,
        vec![
            ("delete_before", Some(Kind::Int)),
            ("excluded_name", Some(Kind::String)),
            ("max_age", Some(Kind::Int)),
            ("min_age", Some(Kind::Int)),
        ]
    );
}

#[test]
fn analyze_workspace_indexes_define_param_defaults_for_later_references() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE PARAM $age VALUE 42;\nSELECT * FROM person WHERE age = $age;".into(),
    );

    let output = analyze_workspace(&workspace);
    let params = &output.sources[&source].inferred_params;

    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, "age");
    assert_eq!(params[0].kind, Some(Kind::Int));
    assert!(!params[0].required);
}

#[test]
fn define_param_defaults_reach_readers_in_other_sources() {
    // A `DEFINE PARAM` installs the value database-side, so it covers every
    // source — not just the ones textually after it. The per-source env
    // resets between sources, which used to leave a cross-source reader
    // recording `unknown [required]`: a host adapter demanding a param the
    // database already supplies.
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE PARAM $default_tier VALUE 'free';".into(),
    );
    let query =
        workspace.add_virtual_source("query".into(), "LET $tier_default = $default_tier;".into());

    let output = analyze_workspace(&workspace);
    let params = &output.sources[&query].inferred_params;

    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, "default_tier");
    assert_eq!(params[0].kind, Some(Kind::String));
    assert!(!params[0].required, "a DEFINE PARAM default covers it");
}

#[test]
fn a_param_with_no_define_param_stays_required() {
    // The counterpart: only a real `DEFINE PARAM` clears `required`. An
    // unrelated definition must not make every param optional.
    let mut workspace = Workspace::default();
    workspace.add_virtual_source(
        "schema".into(),
        "DEFINE PARAM $default_tier VALUE 'free';".into(),
    );
    let query =
        workspace.add_virtual_source("query".into(), "LET $bound = $some_other_param;".into());

    let output = analyze_workspace(&workspace);
    let params = &output.sources[&query].inferred_params;

    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, "some_other_param");
    assert_eq!(params[0].kind, None);
    assert!(params[0].required);
}

#[test]
fn analyze_workspace_uses_define_param_default_for_function_diagnostics() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nDEFINE PARAM $name VALUE 'Ada';\nSELECT string::len($name) AS name_len FROM person;".into(),
    );

    let output = analyze_workspace(&workspace);
    let messages: Vec<_> = output.sources[&source]
        .diagnostics
        .iter()
        .map(|finding| (finding.code().to_string(), finding.message().to_string()))
        .collect();

    assert!(
        messages.iter().all(|(code, _)| code != "E2004"),
        "defined string param should satisfy string::len: {messages:?}"
    );
}

#[test]
fn analyze_workspace_infers_params_from_extended_verified_function_signatures() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        "DEFINE TABLE person;\nSELECT string::lowercase($upper), string::contains($haystack, $needle), array::is_empty($items) FROM person;".into(),
    );

    let output = analyze_workspace(&workspace);
    let param_kinds: Vec<_> = output.sources[&source]
        .inferred_params
        .iter()
        .map(|param| (param.name.as_str(), param.kind.clone()))
        .collect();

    assert_eq!(
        param_kinds,
        vec![
            ("haystack", Some(Kind::String)),
            ("items", Some(Kind::Array(Box::new(Kind::Any), None))),
            ("needle", Some(Kind::String)),
            ("upper", Some(Kind::String)),
        ]
    );
}

#[test]
fn analyze_workspace_exports_param_constraints() {
    let mut workspace = Workspace::default();
    let source = workspace.add_virtual_source(
        "query".into(),
        concat!(
            "DEFINE TABLE person SCHEMAFULL;\n",
            "DEFINE FIELD age ON person TYPE int DEFAULT 0;\n",
            "DEFINE FIELD name ON person TYPE string DEFAULT '';\n",
            "UPDATE person SET age = $age WHERE name = $who;\n",
            "SELECT * FROM person LIMIT $page_size;\n",
            "SELECT type::field($field) FROM person;\n",
            "SELECT * FROM person WHERE age > $min AND $min = 'x';\n",
        )
        .into(),
    );

    let output = analyze_workspace(&workspace);
    let params = &output.sources[&source].inferred_params;
    let get = |name: &str| params.iter().find(|p| p.name == name).unwrap();

    // Assignment position: the field's kind.
    assert_eq!(get("age").kind, Some(Kind::Int));
    // Comparison position: the other side's kind.
    assert_eq!(get("who").kind, Some(Kind::String));
    // LIMIT: an integer with a non-negative domain.
    assert_eq!(get("page_size").kind, Some(Kind::Int));
    assert_eq!(
        get("page_size").domain,
        Some(ValueDomain::Range {
            min: Some(0),
            max: None
        })
    );
    // Value-dependent builtin: string plus the field-path domain.
    assert_eq!(get("field").kind, Some(Kind::String));
    let Some(ValueDomain::OneOf(paths)) = &get("field").domain else {
        panic!("expected OneOf domain, got {:?}", get("field").domain);
    };
    assert!(paths.contains(&surrealdb_types::Value::String("age".into())));
    // Irreconcilable uses: int vs string on $min is 6001.
    assert!(output.sources[&source].diagnostics.iter().any(|finding| {
        finding.code().to_string() == "E6001"
            && finding.message().contains("cannot satisfy this query")
    }));
}

#[test]
fn analyze_workspace_param_compared_to_two_record_tables_is_not_6001() {
    // A host param compared against two different edges' record fields
    // (`in = $p`, then `out = $p`) is satisfiable — records of different
    // tables compare fine, they just aren't equal. No 6001; the param
    // widens to the union of the two record targets.
    let mut workspace = Workspace::default();
    let _source = workspace.add_virtual_source(
        "query".into(),
        concat!(
            "DEFINE TABLE account SCHEMAFULL;\n",
            "DEFINE TABLE team SCHEMAFULL;\n",
            "DEFINE TABLE membership TYPE RELATION IN account OUT team SCHEMAFULL;\n",
            "SELECT * FROM membership WHERE in = $p;\n",
            "SELECT * FROM membership WHERE out = $p;\n",
        )
        .into(),
    );

    let output = analyze_workspace(&workspace);
    assert_eq!(
        codes(&output, 6001),
        0,
        "records of different tables compare fine: {:?}",
        output.diagnostics
    );
}

#[test]
fn analyze_workspace_session_param_guard_then_edge_compare_is_not_6001() {
    // The workshop pattern: `IF $auth = NONE ...` (a `none` comparison)
    // then `WHERE in = $auth` (a record comparison). `$auth` is a session
    // param and the NONE guard is an existence check — not a demand that it
    // be `none`. No 6001.
    let mut workspace = Workspace::default();
    let _source = workspace.add_virtual_source(
        "query".into(),
        concat!(
            "DEFINE TABLE account SCHEMAFULL;\n",
            "DEFINE TABLE org SCHEMAFULL;\n",
            "DEFINE TABLE employee_of TYPE RELATION IN account OUT org SCHEMAFULL;\n",
            "DEFINE FUNCTION fn::guard() {\n",
            "  IF $auth = NONE THEN RETURN false END;\n",
            "  LET $rows = SELECT VALUE id FROM employee_of WHERE in = $auth;\n",
            "  RETURN array::len($rows) > 0;\n",
            "};\n",
        )
        .into(),
    );

    let output = analyze_workspace(&workspace);
    assert_eq!(
        codes(&output, 6001),
        0,
        "session-param NONE guard then edge compare is satisfiable: {:?}",
        output.diagnostics
    );
}
