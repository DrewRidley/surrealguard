//! The contracts a *write* is held to beyond its declared type, end to end.
//!
//! 2038 — a constant written to a field satisfies the field's `ASSERT` — is
//! the same fold 2037 applies to a `DEFAULT`, asked of every constant a
//! statement writes. It has one positive shape (the predicate folds to a
//! definite `false` under `$value = <constant>`) and several near-misses that
//! must stay silent: an allowed constant, a non-constant, a predicate the
//! folder cannot prove, and a value the type contract (2001) already rejected
//! — one finding per contract violated, and the type contract comes first.
//!
//! 7012 — a blocking or non-deterministic call in a field clause that re-runs
//! it — has a positive shape per clause and a near-miss per clause: `DEFAULT
//! time::now()` / `DEFAULT rand::uuid()` are the created-at and id idioms, and
//! `VALUE time::now()` is the updated-at idiom; none may fire.

use surrealguard_diagnostics::Finding;
use surrealguard_workspace::{analyze_workspace, Workspace};

/// Every finding a schema and a query raise together.
fn findings(schema: &str, query: &str) -> Vec<Finding> {
    let mut workspace = Workspace::default();
    workspace.add_virtual_source("schema".into(), schema.to_string());
    workspace.add_virtual_source("query".into(), query.to_string());
    let diagnostics = analyze_workspace(&workspace).diagnostics;
    assert!(
        !diagnostics
            .iter()
            .any(|finding| finding.code().prefix() == 'S'),
        "input does not parse:\n{schema}\n{query}\n{diagnostics:#?}"
    );
    diagnostics
}

fn codes(findings: &[Finding], code: u16) -> Vec<&Finding> {
    findings
        .iter()
        .filter(|finding| finding.code().number() == code)
        .collect()
}

// -- 2038 ------------------------------------------------------------------

const STATUS: &str = "\
DEFINE TABLE t SCHEMAFULL;
DEFINE FIELD status ON t TYPE string ASSERT $value IN ['active', 'inactive'];
DEFINE FIELD age ON t TYPE int ASSERT $value >= 0 AND $value < 150;
DEFINE FIELD tag ON t TYPE option<string> ASSERT $value IN ['a', 'b'];
";

#[test]
fn a_rejected_constant_fires_at_every_write_form() {
    for query in [
        "CREATE t SET status = 'activ';",
        "UPDATE t:one SET status = 'activ';",
        "UPSERT t:one SET status = 'activ';",
        "CREATE t CONTENT { status: 'activ' };",
        "UPDATE t:one MERGE { status: 'activ' };",
        "UPDATE t:one REPLACE { status: 'activ' };",
        "INSERT INTO t { status: 'activ' };",
        "INSERT INTO t [{ status: 'activ' }];",
        "INSERT INTO t (status) VALUES ('activ');",
        "UPDATE t:one PATCH [{ op: 'replace', path: '/status', value: 'activ' }];",
        "UPDATE t:one PATCH [{ op: 'add', path: '/status', value: 'activ' }];",
    ] {
        let all = findings(STATUS, query);
        let fired = codes(&all, 2038);
        assert_eq!(fired.len(), 1, "{query}\n{all:#?}");
        let finding = fired[0];
        assert_eq!(
            finding.message(),
            "`status`'s ASSERT rejects `'activ'`",
            "{query}"
        );
        assert_eq!(
            &query[finding.span().range().start() as usize..finding.span().range().end() as usize],
            "'activ'",
            "{query}: the finding sits on the written constant"
        );
        assert!(
            finding
                .related()
                .iter()
                .any(|related| related.message.contains("ASSERT is defined here")),
            "{query}: points at the ASSERT clause\n{finding:#?}"
        );
        // …and the type contract did not fire: the value *is* a string.
        assert!(codes(&all, 2001).is_empty(), "{query}\n{all:#?}");
    }
}

#[test]
fn a_relate_write_is_a_write() {
    const EDGE: &str = "\
DEFINE TABLE person SCHEMAFULL;
DEFINE TABLE post SCHEMAFULL;
DEFINE TABLE likes SCHEMAFULL TYPE RELATION FROM person TO post;
DEFINE FIELD weight ON likes TYPE int ASSERT $value > 0;
";
    for query in [
        "RELATE person:a->likes->post:b SET weight = 0;",
        "RELATE person:a->likes->post:b CONTENT { weight: 0 };",
    ] {
        let all = findings(EDGE, query);
        assert_eq!(codes(&all, 2038).len(), 1, "{query}\n{all:#?}");
    }
}

#[test]
fn comparison_and_boolean_predicates_fold_too() {
    let all = findings(STATUS, "CREATE t SET age = 200;");
    assert_eq!(codes(&all, 2038).len(), 1, "{all:#?}");
    let all = findings(STATUS, "CREATE t SET age = -1;");
    assert_eq!(codes(&all, 2038).len(), 1, "{all:#?}");
    let all = findings(STATUS, "CREATE t SET age = 42;");
    assert!(codes(&all, 2038).is_empty(), "{all:#?}");
}

#[test]
fn an_allowed_constant_is_silent() {
    for query in [
        "CREATE t SET status = 'active';",
        "CREATE t CONTENT { status: 'inactive' };",
        "INSERT INTO t (status) VALUES ('active');",
        "UPDATE t:one PATCH [{ op: 'replace', path: '/status', value: 'active' }];",
    ] {
        let all = findings(STATUS, query);
        assert!(codes(&all, 2038).is_empty(), "{query}\n{all:#?}");
    }
}

#[test]
fn a_non_constant_is_never_judged() {
    for query in [
        "CREATE t SET status = $p;",
        "CREATE t SET status = string::lowercase('ACTIV');",
        "CREATE t CONTENT { status: $p };",
        "INSERT INTO t (status) VALUES ($p);",
        "UPDATE t:one PATCH [{ op: 'replace', path: '/status', value: $p }];",
        "CREATE t SET status = (SELECT VALUE status FROM ONLY t:other);",
    ] {
        let all = findings(STATUS, query);
        assert!(codes(&all, 2038).is_empty(), "{query}\n{all:#?}");
    }
}

#[test]
fn a_predicate_the_folder_cannot_prove_is_silent() {
    const OPAQUE: &str = "\
DEFINE TABLE t SCHEMAFULL;
DEFINE FIELD name ON t TYPE string ASSERT string::len($value) > 3;
DEFINE FIELD code ON t TYPE string ASSERT $value IN $allowed;
DEFINE FIELD other ON t TYPE int;
DEFINE FIELD linked ON t TYPE int ASSERT $value = $this.other;
";
    for query in [
        "CREATE t SET name = 'ab';",
        "CREATE t SET code = 'zzz';",
        "CREATE t SET linked = 1;",
    ] {
        let all = findings(OPAQUE, query);
        assert!(codes(&all, 2038).is_empty(), "{query}\n{all:#?}");
    }
}

#[test]
fn a_value_the_type_contract_rejected_is_reported_once() {
    // `1` is not a string: 2001 owns this write, and 2038 must not pile on
    // even though `1 IN ['active', 'inactive']` also folds to false.
    for query in [
        "CREATE t SET status = 1;",
        "CREATE t CONTENT { status: 1 };",
        "INSERT INTO t (status) VALUES (1);",
        "UPDATE t:one PATCH [{ op: 'replace', path: '/status', value: 1 }];",
    ] {
        let all = findings(STATUS, query);
        assert_eq!(codes(&all, 2001).len(), 1, "{query}\n{all:#?}");
        assert!(codes(&all, 2038).is_empty(), "{query}\n{all:#?}");
    }
}

#[test]
fn none_into_an_optional_field_skips_the_assert() {
    // The engine does not evaluate an optional field's ASSERT for NONE.
    let all = findings(STATUS, "CREATE t SET tag = NONE;");
    assert!(codes(&all, 2038).is_empty(), "{all:#?}");
    assert!(codes(&all, 2001).is_empty(), "{all:#?}");
    // …but a wrong non-NONE constant is still wrong.
    let all = findings(STATUS, "CREATE t SET tag = 'c';");
    assert_eq!(codes(&all, 2038).len(), 1, "{all:#?}");
}

#[test]
fn the_default_check_and_the_write_check_agree() {
    // 2037 and 2038 are one fold: what rejects the DEFAULT rejects the write.
    const BOTH: &str = "\
DEFINE TABLE t SCHEMAFULL;
DEFINE FIELD status ON t TYPE string DEFAULT 'activ' ASSERT $value IN ['active', 'inactive'];
DEFINE FIELD tag ON t TYPE option<string> DEFAULT NONE ASSERT $value IN ['a', 'b'];
";
    let all = findings(BOTH, "CREATE t SET status = 'activ';");
    assert_eq!(codes(&all, 2037).len(), 1, "{all:#?}");
    assert_eq!(codes(&all, 2038).len(), 1, "{all:#?}");
    // A `DEFAULT NONE` on an optional field is not a DEFAULT the ASSERT sees.
    let all = findings(BOTH, "CREATE t SET tag = 'a';");
    assert_eq!(codes(&all, 2037).len(), 1, "{all:#?}");
    assert!(
        codes(&all, 2037)[0].message().contains("`status`"),
        "{all:#?}"
    );
}

// -- PATCH typing ---------------------------------------------------------

#[test]
fn a_patch_add_or_replace_value_is_typed_against_its_field() {
    const NESTED: &str = "\
DEFINE TABLE t SCHEMAFULL;
DEFINE FIELD age ON t TYPE int;
DEFINE FIELD address ON t TYPE object;
DEFINE FIELD address.city ON t TYPE string;
DEFINE FIELD tags ON t TYPE array<string>;
";
    let all = findings(
        NESTED,
        "UPDATE t:one PATCH [{ op: 'replace', path: '/age', value: 'old' }];",
    );
    assert_eq!(codes(&all, 2001).len(), 1, "{all:#?}");
    assert!(codes(&all, 2001)[0].message().contains("`age`"));

    // A nested pointer is a dotted field path.
    let all = findings(
        NESTED,
        "UPDATE t:one PATCH [{ op: 'add', path: '/address/city', value: 7 }];",
    );
    assert_eq!(codes(&all, 2001).len(), 1, "{all:#?}");
    assert!(codes(&all, 2001)[0].message().contains("`address.city`"));

    // An unknown field is the same unknown-field finding a CONTENT key gets.
    let all = findings(
        NESTED,
        "UPDATE t:one PATCH [{ op: 'replace', path: '/agee', value: 1 }];",
    );
    assert_eq!(codes(&all, 1002).len(), 1, "{all:#?}");

    // Well-typed, array positions, and non-writing ops are all silent.
    for query in [
        "UPDATE t:one PATCH [{ op: 'replace', path: '/age', value: 30 }];",
        "UPDATE t:one PATCH [{ op: 'add', path: '/tags/0', value: 1 }];",
        "UPDATE t:one PATCH [{ op: 'add', path: '/tags/-', value: 1 }];",
        "UPDATE t:one PATCH [{ op: 'test', path: '/age', value: 'old' }];",
        "UPDATE t:one PATCH [{ op: 'remove', path: '/age' }];",
        "UPDATE t:one PATCH [{ op: 'replace', path: '/age', value: $p }];",
    ] {
        let all = findings(NESTED, query);
        assert!(codes(&all, 2001).is_empty(), "{query}\n{all:#?}");
        assert!(codes(&all, 1002).is_empty(), "{query}\n{all:#?}");
    }
}

// -- 7012 ------------------------------------------------------------------

fn field_clause(clause: &str) -> Vec<Finding> {
    findings(
        &format!("DEFINE TABLE t SCHEMAFULL;\nDEFINE FIELD f ON t {clause};\n"),
        "",
    )
}

#[test]
fn a_fresh_value_in_a_clause_that_reruns_fires() {
    for (clause, path) in [
        ("TYPE uuid VALUE rand::uuid()", "rand::uuid"),
        ("TYPE int VALUE rand::int(0, 9)", "rand::int"),
        ("TYPE float VALUE rand()", "rand"),
        ("TYPE int VALUE sequence::nextval('s')", "sequence::nextval"),
        ("TYPE uuid COMPUTED rand::uuid()", "rand::uuid"),
        ("TYPE datetime COMPUTED time::now()", "time::now"),
        // Reached through parentheses, a method chain, or an argument.
        ("TYPE string VALUE (rand::uuid()).to_string()", "rand::uuid"),
        (
            "TYPE string VALUE string::concat('a', rand::string(4))",
            "rand::string",
        ),
    ] {
        let all = field_clause(clause);
        let fired = codes(&all, 7012);
        assert_eq!(fired.len(), 1, "{clause}\n{all:#?}");
        assert!(
            fired[0]
                .message()
                .starts_with(&format!("`{path}` gives `f` a different value")),
            "{clause}: {}",
            fired[0].message()
        );
    }
}

#[test]
fn a_blocking_call_fires_in_every_clause() {
    for clause in [
        "TYPE string DEFAULT http::get('https://x.test')",
        "TYPE string VALUE http::get('https://x.test')",
        "TYPE string COMPUTED http::get('https://x.test')",
        "TYPE string ASSERT http::get('https://x.test') = $value",
        "TYPE string VALUE sleep(1s)",
    ] {
        let all = field_clause(clause);
        assert_eq!(codes(&all, 7012).len(), 1, "{clause}\n{all:#?}");
    }
    let all = field_clause("TYPE string VALUE http::get('https://x.test')");
    assert_eq!(
        codes(&all, 7012)[0].message(),
        "`http::get` runs on every write to this row"
    );
}

#[test]
fn the_run_once_idioms_stay_silent() {
    for clause in [
        // The created-at and id idioms: DEFAULT runs once.
        "TYPE datetime DEFAULT time::now()",
        "TYPE uuid DEFAULT rand::uuid()",
        "TYPE int DEFAULT sequence::nextval('s')",
        // The updated-at idiom: the clock moving on every write is the point.
        "TYPE datetime VALUE time::now()",
        "TYPE datetime DEFAULT time::now() VALUE time::now()",
        // A clock in an ASSERT is a bound, not a stored value.
        "TYPE datetime ASSERT $value < time::now()",
        // Deterministic calls are not the contract's business.
        "TYPE string VALUE string::uppercase($value)",
        "TYPE string COMPUTED string::concat('a', 'b')",
    ] {
        let all = field_clause(clause);
        assert!(codes(&all, 7012).is_empty(), "{clause}\n{all:#?}");
    }
}
