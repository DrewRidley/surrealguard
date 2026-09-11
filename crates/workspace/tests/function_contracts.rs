//! The `DEFINE FUNCTION` body contracts, and the one condition-position rule.
//!
//! Two contracts, both established against a live SurrealDB 3.2.3 before they
//! were written down:
//!
//! * **6008 — a param a function body reads is one that something binds.** An
//!   undeclared name in a body is not a parse error and not a call-time error;
//!   it evaluates to `NONE`. `DEFINE FUNCTION fn::undeclared() { RETURN $nope; }`
//!   defines cleanly and `fn::undeclared()` returns `NONE`. So the finding is a
//!   Warning: the function runs and quietly computes the wrong answer.
//!
//! * **2005 — a condition position expects a boolean.** Engine-verified that a
//!   condition takes the value's *truthiness* rather than requiring a bool:
//!   `WHERE "yes"` matches every row, `WHERE ""` none; `IF "yes"` takes the
//!   then-branch; `ASSERT ""` fails. Nothing errors on a non-boolean. So a kind
//!   that merely *may* be non-boolean (`bool | string`) satisfies the contract
//!   and stays silent — in permission predicates and in every other condition
//!   position alike, because it is one rule and not a per-position denylist.
//!
//! Every positive case has its near-miss beside it: a check that fires on the
//! intended form is worse than one that misses the accident.

use surrealguard_diagnostics::Finding;
use surrealguard_workspace::{analyze_workspace, Workspace, WorkspaceAnalysis};

fn analyze(source: &str) -> WorkspaceAnalysis {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source("schema".into(), source.into());
    analyze_workspace(&workspace)
}

fn with_code(output: &WorkspaceAnalysis, code: u16) -> Vec<&Finding> {
    output
        .diagnostics
        .iter()
        .filter(|finding| finding.code().number() == code)
        .collect()
}

fn fires(source: &str, code: u16) -> bool {
    !with_code(&analyze(source), code).is_empty()
}

fn messages(source: &str, code: u16) -> Vec<String> {
    with_code(&analyze(source), code)
        .into_iter()
        .map(|finding| finding.message().to_string())
        .collect()
}

fn helps(source: &str, code: u16) -> Vec<String> {
    with_code(&analyze(source), code)
        .into_iter()
        .flat_map(|finding| finding.help().iter().map(|help| help.message.clone()))
        .collect()
}

// --- 6008: a param a function body reads is one that something binds --------

/// The demo verbatim. `$parm` is declared, `$param` is read — so the `IF` is
/// `NONE = "admin"`, always false, and the function always takes its `ELSE`.
#[test]
fn the_demo_function_reports_its_undeclared_param() {
    let source = r#"
DEFINE FUNCTION fn::get_permissions($parm: any) {
    IF $param = "admin" {
        RETURN true;
    }
    ELSE {
        RETURN "5";
    }
};
DEFINE FIELD email ON person TYPE option<string>
    PERMISSIONS FOR SELECT WHERE fn::get_permissions(5);
"#;
    let messages = messages(source, 6008);
    assert_eq!(messages.len(), 1, "one undeclared param: {messages:?}");
    assert!(
        messages[0].contains("$param") && messages[0].contains("fn::get_permissions"),
        "names the param and the function: {messages:?}"
    );
}

/// A one-edit typo gets the declared name back.
#[test]
fn a_near_miss_param_suggests_the_declared_one() {
    let helps = helps(
        "DEFINE FUNCTION fn::f($parm: any) { RETURN $param; };",
        6008,
    );
    assert_eq!(
        helps,
        vec!["did you mean `$parm`?".to_string()],
        "the declared spelling is offered"
    );
}

/// Nothing close enough to suggest: say what the engine actually does instead
/// of guessing a name.
#[test]
fn a_far_param_explains_the_runtime_behaviour_instead() {
    let helps = helps(
        "DEFINE FUNCTION fn::f($parm: any) { RETURN $wildlydifferent; };",
        6008,
    );
    assert_eq!(
        helps,
        vec!["an unbound parameter in a function body evaluates to NONE".to_string()],
    );
}

/// The whole point of the near-miss: the correctly spelled body is silent.
#[test]
fn a_correctly_spelled_param_is_silent() {
    assert!(!fires(
        "DEFINE FUNCTION fn::f($parm: any) { RETURN $parm; };",
        6008
    ));
}

/// `$auth` and `$session` come from the session in every context, engine-verified
/// inside a body (`$session` is an object there; `$auth` is `NONE` unauthenticated).
#[test]
fn session_params_are_silent() {
    assert!(!fires(
        "DEFINE FUNCTION fn::f() { RETURN [$auth, $session, $token, $access, $scope]; };",
        6008
    ));
}

/// The document context params. The engine accepts all of them inside a body
/// (they evaluate to `NONE` outside their context), so the analyzer is not
/// entitled to call one unbound here.
#[test]
fn document_context_params_are_silent() {
    assert!(!fires(
        "DEFINE FUNCTION fn::f() { RETURN [$this, $parent, $value, $before, $after, $event, $input]; };",
        6008
    ));
}

/// `DEFINE PARAM $globalp VALUE "hello"` then reading `$globalp` in a body
/// returns `'hello'` on the engine.
#[test]
fn a_define_param_reference_is_silent() {
    assert!(!fires(
        r#"DEFINE PARAM $globalp VALUE "hello";
DEFINE FUNCTION fn::f() { RETURN $globalp; };"#,
        6008
    ));
}

/// A `DEFINE PARAM` *after* the function still binds the name at call time —
/// the catalog is workspace-global, not source-ordered, for params.
#[test]
fn a_later_define_param_is_silent() {
    assert!(!fires(
        r#"DEFINE FUNCTION fn::f() { RETURN $globalp; };
DEFINE PARAM $globalp VALUE "hello";"#,
        6008
    ));
}

/// The body binds the name itself.
#[test]
fn a_body_let_is_silent() {
    assert!(!fires(
        r#"DEFINE FUNCTION fn::f() { LET $inner = "i"; RETURN $inner; };"#,
        6008
    ));
}

/// Reading a body `LET` before it runs is 6004's contract. 6008 must not also
/// fire on the same mistake — one contract, one code.
#[test]
fn a_body_let_read_early_is_not_also_6008() {
    assert!(!fires(
        r#"DEFINE FUNCTION fn::f() { RETURN $inner; LET $inner = "i"; };"#,
        6008
    ));
}

#[test]
fn a_for_binding_is_silent() {
    assert!(!fires(
        "DEFINE FUNCTION fn::f($xs: array) { FOR $x IN $xs { RETURN $x; }; };",
        6008
    ));
}

#[test]
fn a_closure_param_is_silent() {
    assert!(!fires(
        "DEFINE FUNCTION fn::f() { LET $g = |$x: int| { RETURN $x + 1; }; RETURN $g(1); };",
        6008
    ));
}

/// A body reads the *caller's* scope: engine-verified that
/// `LET $outer = "x"; DEFINE FUNCTION fn::f() { RETURN $outer; }` returns `'x'`.
#[test]
fn an_enclosing_let_is_silent() {
    assert!(!fires(
        r#"LET $outer = "x";
DEFINE FUNCTION fn::f() { RETURN $outer; };"#,
        6008
    ));
}

/// A function with no body at all has nothing to check.
#[test]
fn a_bodyless_function_is_silent() {
    assert!(!fires("DEFINE FUNCTION fn::f($a: int) {};", 6008));
}

/// Several undeclared names are several findings — one per use site.
#[test]
fn each_undeclared_param_reports_once_per_site() {
    let messages = messages(
        "DEFINE FUNCTION fn::f($a: int) { RETURN [$nope, $alsonope]; };",
        6008,
    );
    assert_eq!(messages.len(), 2, "{messages:?}");
}

// --- 2005: one condition-position rule, engine-verified --------------------

/// The demo's predicate is `bool | string`. The engine takes a condition's
/// truthiness — `"5"` ALLOWS, it does not error and does not deny — so a union
/// that merely *may* be non-boolean satisfies the contract.
#[test]
fn a_union_permission_predicate_that_may_be_bool_is_silent() {
    assert!(!fires(
        r#"DEFINE FUNCTION fn::gp($p: any) { IF $p = "admin" { RETURN true; } ELSE { RETURN "5"; } };
DEFINE TABLE person SCHEMAFULL;
DEFINE FIELD email ON person TYPE option<string>
    PERMISSIONS FOR SELECT WHERE fn::gp(5);"#,
        2005
    ));
}

#[test]
fn a_plainly_bool_permission_predicate_is_silent() {
    assert!(!fires(
        r#"DEFINE TABLE person SCHEMAFULL;
DEFINE FIELD owner ON person TYPE option<record<person>>;
DEFINE FIELD email ON person TYPE option<string>
    PERMISSIONS FOR SELECT WHERE owner = $auth;"#,
        2005
    ));
}

/// `any` is unresolvable, not wrong. It must stay silent everywhere.
#[test]
fn an_any_typed_predicate_is_silent() {
    assert!(!fires(
        r#"DEFINE FUNCTION fn::whatever($p: any) { RETURN $p; };
DEFINE TABLE person SCHEMAFULL;
DEFINE FIELD email ON person TYPE option<string>
    PERMISSIONS FOR SELECT WHERE fn::whatever(5);"#,
        2005
    ));
}

/// The same union, in the other three positions 2005 covers. One rule, so all
/// four agree — this is what keeps the contract from becoming a permissions
/// special case.
#[test]
fn a_union_that_may_be_bool_is_silent_in_every_condition_position() {
    let prelude = r#"DEFINE FUNCTION fn::gp($p: any) { IF $p = "admin" { RETURN true; } ELSE { RETURN "5"; } };
DEFINE TABLE person SCHEMAFULL;
DEFINE FIELD name ON person TYPE option<string>;
"#;
    for position in [
        "SELECT * FROM person WHERE fn::gp(5);",
        "UPDATE person SET name = 'x' WHERE fn::gp(5);",
        "IF fn::gp(5) { RETURN 1; };",
        "DEFINE FIELD flag ON person TYPE option<string> ASSERT fn::gp(5);",
    ] {
        let source = format!("{prelude}{position}");
        assert!(
            !fires(&source, 2005),
            "2005 should stay silent for a `bool | string` condition in: {position}"
        );
    }
}
