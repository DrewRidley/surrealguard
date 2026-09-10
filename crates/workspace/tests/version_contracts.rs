//! The 8xxx contracts: what the configured SurrealDB release has.
//!
//! Every case is a fire/near-miss pair: the same construct on a target that
//! lacks it and on one that has it, plus the unconfigured default (the latest
//! release), which must never fire. The facts behind each pinned version are
//! cited in `crates/workspace/src/analyzer/version.rs`.

use surrealguard_workspace::config::WorkspaceConfig;
use surrealguard_workspace::{analyze_query, Workspace};

/// `(code, message)` for every finding of `query` under `surrealdb_version`
/// (`None` leaves the key unset).
fn findings(version: Option<&str>, query: &str) -> Vec<(u16, String)> {
    let config = match version {
        Some(version) => WorkspaceConfig::from_toml_str(&format!(
            "[analysis]\nsurrealdb_version = \"{version}\"\n"
        ))
        .expect("config parses"),
        None => WorkspaceConfig::default(),
    };
    let mut workspace = Workspace::new(config);
    let mut out: Vec<(u16, String)> = analyze_query(&mut workspace, query)
        .diagnostics
        .iter()
        .map(|finding| (finding.code().number(), finding.message().to_string()))
        .collect();
    out.sort();
    out
}

fn codes(version: Option<&str>, query: &str) -> Vec<u16> {
    findings(version, query)
        .into_iter()
        .map(|(code, _)| code)
        .collect()
}

fn message_of(version: Option<&str>, query: &str, code: u16) -> String {
    findings(version, query)
        .into_iter()
        .find(|(found, _)| *found == code)
        .map_or_else(
            || panic!("{query:?} on {version:?} did not report {code}"),
            |(_, message)| message,
        )
}

// ---------------------------------------------------------------------------
// 8001 — functions
// ---------------------------------------------------------------------------

#[test]
fn a_function_added_later_than_the_target_is_8001() {
    // `file::*` and `set::*` arrived in 3.0.
    assert_eq!(codes(Some("2.2"), "RETURN set::len([1, 2]);"), vec![8001]);
    let message = message_of(Some("2.2"), "RETURN set::len([1, 2]);", 8001);
    assert!(message.contains("SurrealDB 2.2"), "{message}");
    assert!(message.contains("added in 3.0"), "{message}");

    // Near misses: a target that has it, and no target at all.
    assert_eq!(
        codes(Some("3.0"), "RETURN set::len([1, 2]);"),
        Vec::<u16>::new()
    );
    assert_eq!(codes(None, "RETURN set::len([1, 2]);"), Vec::<u16>::new());
    // A function available in 2.2 is silent on 2.2.
    assert_eq!(
        codes(Some("2.2"), "RETURN array::len([1]);"),
        Vec::<u16>::new()
    );
    assert_eq!(
        codes(Some("2.2"), "RETURN object::is_empty({});"),
        Vec::<u16>::new(),
        "object::is_empty arrived in 2.2 itself"
    );
    assert_eq!(
        codes(Some("2.1"), "RETURN object::is_empty({});"),
        vec![8001]
    );
}

#[test]
fn a_major_only_target_is_the_newest_of_that_line() {
    // `"2"` is every 2.x: nothing added within 2.x is missing from it...
    assert_eq!(
        codes(Some("2"), "RETURN rand::duration(1s, 2s);"),
        Vec::<u16>::new()
    );
    assert_eq!(
        codes(Some("2"), "RETURN array::sort_lexical([]);"),
        Vec::<u16>::new()
    );
    // ...but 3.0 additions are.
    assert_eq!(
        codes(Some("2"), "RETURN file::exists('b', 'k');"),
        vec![8001]
    );
    // And a 2.x-specific target sees the 2.3 additions as missing.
    assert_eq!(
        codes(Some("2.2"), "RETURN rand::duration(1s, 2s);"),
        vec![8001]
    );
}

#[test]
fn a_renamed_function_reports_the_spelling_the_target_uses() {
    // The 3.x spelling on a 2.x target names the old spelling...
    let message = message_of(Some("2.2"), "RETURN type::is_record(user:one);", 8001);
    assert!(message.contains("`type::is::record`"), "{message}");
    // ...and the 2.x spelling on a 3.x target names the new one.
    let message = message_of(Some("3.0"), "RETURN type::is::record(user:one);", 8001);
    assert!(
        message.contains("renamed to `type::is_record` in 3.0"),
        "{message}"
    );
    // A renamed call is still analyzed as that function: no 5001, and no
    // second finding.
    assert_eq!(
        codes(Some("3.0"), "RETURN type::is::record(user:one);"),
        vec![8001]
    );
    assert_eq!(
        codes(Some("3"), "RETURN time::from::millis(1);"),
        vec![8001]
    );
    assert_eq!(
        codes(Some("3"), "RETURN string::startsWith('ab', 'a');"),
        vec![8001]
    );
    let message = message_of(Some("3"), "RETURN string::startsWith('ab', 'a');", 8001);
    assert!(
        message.contains("`string::starts_with`") && message.contains("2.0"),
        "{message}"
    );

    // Near misses: the spelling the target has.
    assert_eq!(
        codes(Some("2.2"), "RETURN type::is::record(user:one);"),
        Vec::<u16>::new()
    );
    assert_eq!(
        codes(Some("2.2"), "RETURN time::from::millis(1);"),
        Vec::<u16>::new()
    );
    assert_eq!(
        codes(Some("2.2"), "RETURN time::from::ulid(rand::ulid());"),
        Vec::<u16>::new()
    );
    assert_eq!(
        codes(Some("3.0"), "RETURN type::is_record(user:one);"),
        Vec::<u16>::new()
    );
    assert_eq!(
        codes(Some("3.0"), "RETURN time::from_millis(1);"),
        Vec::<u16>::new()
    );
    // Unconfigured means the latest release, which refuses to parse a retired
    // spelling: each is a plain 5001 (not 8001 — there is no target to hold it
    // against) carrying the rename as its help, and the current spelling is
    // clean.
    for retired in [
        "RETURN type::is::record(user:one);",
        "RETURN time::from::ulid(rand::ulid());",
        "RETURN string::startsWith('ab', 'a');",
    ] {
        assert_eq!(codes(None, retired), vec![5001], "{retired}");
    }
    assert_eq!(
        codes(None, "RETURN type::is_record(user:one);"),
        Vec::<u16>::new()
    );
}

#[test]
fn a_function_removed_without_a_replacement_is_8001_past_the_removal() {
    assert_eq!(codes(Some("3.0"), "RETURN rand::guid();"), vec![8001]);
    let message = message_of(Some("3.0"), "RETURN rand::guid();", 8001);
    assert!(message.contains("removed in 3.0"), "{message}");
    assert_eq!(
        codes(Some("2.3"), "RETURN rand::guid();"),
        Vec::<u16>::new()
    );
    // Unconfigured is the latest release, where the name is gone: 5001.
    assert_eq!(codes(None, "RETURN rand::guid();"), vec![5001]);
}

#[test]
fn version_facts_reach_calls_nested_in_bodies_and_subqueries() {
    let body = "DEFINE FUNCTION fn::f() { RETURN set::len([1]); };";
    assert_eq!(
        codes(Some("2.2"), body),
        vec![8001],
        "{:?}",
        findings(Some("2.2"), body)
    );
    // A projection subquery over a real table (projections over an array
    // literal source are not analyzed at all today — 5001 is silent there
    // too, so it is not this check's gap).
    let subquery = "DEFINE TABLE t SCHEMALESS;\n\
                    SELECT (RETURN file::exists('b', 'k')) AS x FROM t LIMIT 1;";
    assert_eq!(
        codes(Some("2.2"), subquery)
            .into_iter()
            .filter(|code| *code == 8001)
            .count(),
        1,
        "{:?}",
        findings(Some("2.2"), subquery)
    );
    let closure = "RETURN [1].map(|$v| set::len([$v]));";
    assert_eq!(
        codes(Some("2.2"), closure),
        vec![8001],
        "{:?}",
        findings(Some("2.2"), closure)
    );
}

// ---------------------------------------------------------------------------
// 8003 — syntax the target predates
// ---------------------------------------------------------------------------

#[test]
fn syntax_added_after_the_target_is_8003() {
    // Closures, UPSERT, record-id ranges, optional chaining: 2.0.
    for query in [
        "RETURN |$x| $x + 1;",
        "UPSERT t SET a = 1;",
        "RETURN person:1..5;",
        "SELECT foo?.bar FROM t;",
        "SELECT foo.{a, b} FROM t;",
        "DEFINE ACCESS a ON DATABASE TYPE RECORD;",
    ] {
        assert!(
            codes(Some("1.5"), query).contains(&8003),
            "{query} should be 8003 on 1.5: {:?}",
            findings(Some("1.5"), query)
        );
        assert!(
            !codes(Some("2.0"), query).contains(&8003),
            "{query} is 2.0 syntax: {:?}",
            findings(Some("2.0"), query)
        );
        assert!(!codes(None, query).contains(&8003), "{query} unconfigured");
    }
    let message = message_of(Some("1.5"), "RETURN |$x| $x + 1;", 8003);
    assert!(message.contains("requires SurrealDB 2.0"), "{message}");
    assert!(message.contains("target is 1.5"), "{message}");

    // Recursion: 2.1.
    assert!(codes(Some("2.0"), "SELECT ->knows.{1..3} FROM t;").contains(&8003));
    assert!(!codes(Some("2.1"), "SELECT ->knows.{1..3} FROM t;").contains(&8003));

    // REFERENCE fields and `<~`: 2.2.
    let reference = "DEFINE FIELD org ON team TYPE record<organization> REFERENCE;";
    assert!(codes(Some("2.1"), reference).contains(&8003));
    assert!(!codes(Some("2.2"), reference).contains(&8003));

    // COMPUTED, DEFINE SEQUENCE / BUCKET: 3.0.
    for query in [
        "DEFINE FIELD total ON t COMPUTED 1 + 1;",
        "DEFINE SEQUENCE s;",
        "DEFINE BUCKET b BACKEND 'memory';",
    ] {
        assert!(codes(Some("2.3"), query).contains(&8003), "{query} on 2.3");
        assert!(!codes(Some("3.0"), query).contains(&8003), "{query} on 3.0");
        assert!(!codes(None, query).contains(&8003), "{query} unconfigured");
    }

    // ASSERT/DEFAULT on `id`: 3.2.
    let id_default = "DEFINE FIELD id ON t DEFAULT rand::ulid();";
    assert!(codes(Some("3.1"), id_default).contains(&8003));
    assert!(!codes(Some("3.2"), id_default).contains(&8003));
    assert!(!codes(Some("3.1"), "DEFINE FIELD name ON t DEFAULT 'x';").contains(&8003));
}

// ---------------------------------------------------------------------------
// 8002 — syntax the target removed
// ---------------------------------------------------------------------------

#[test]
fn syntax_removed_before_the_target_is_8002() {
    for (query, replacement) in [
        ("DEFINE SCOPE account SESSION 24h;", "DEFINE ACCESS"),
        (
            "DEFINE TOKEN t ON DATABASE TYPE HS512 VALUE 'x';",
            "DEFINE ACCESS",
        ),
        ("RETURN <future> { 1 + 1 };", "COMPUTED"),
        ("SELECT * FROM t WHERE name ~ 'bob';", "string::similarity"),
        (
            "DEFINE INDEX i ON t FIELDS name SEARCH ANALYZER a BM25;",
            "FULLTEXT ANALYZER",
        ),
    ] {
        let on_three = findings(Some("3.0"), query);
        assert!(
            on_three.iter().any(|(code, _)| *code == 8002),
            "{query} should be 8002 on 3.0: {on_three:?}"
        );
        let message = message_of(Some("3.0"), query, 8002);
        assert!(message.contains("removed in SurrealDB 3.0"), "{message}");
        let help = {
            let mut workspace = Workspace::new(
                WorkspaceConfig::from_toml_str("[analysis]\nsurrealdb_version = \"3.0\"\n")
                    .unwrap(),
            );
            analyze_query(&mut workspace, query)
                .diagnostics
                .iter()
                .find(|finding| finding.code().number() == 8002)
                .map(|finding| {
                    finding
                        .help()
                        .iter()
                        .map(|help| help.message.clone())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default()
        };
        assert!(help.contains(replacement), "{query}: help {help:?}");
        // Near misses: a 2.x target still has all of these; unconfigured is silent.
        assert!(
            !codes(Some("2.3"), query).contains(&8002),
            "{query} on 2.3: {:?}",
            findings(Some("2.3"), query)
        );
        assert!(!codes(None, query).contains(&8002), "{query} unconfigured");
    }

    // The 3.0 spelling of the same index clause is 8003 on a 2.x target.
    let fulltext = "DEFINE INDEX i ON t FIELDS name FULLTEXT ANALYZER a BM25;";
    assert!(codes(Some("2.3"), fulltext).contains(&8003));
    assert!(!codes(Some("3.0"), fulltext).contains(&8003));
}

// ---------------------------------------------------------------------------
// 4012 — OMIT without a wildcard projection
// ---------------------------------------------------------------------------

#[test]
fn omit_under_an_explicit_projection_is_4012() {
    // The table exists and the select is bounded, so the only thing left to
    // say about each query is the OMIT.
    let count = |query: &str| {
        let query = format!("DEFINE TABLE t SCHEMALESS; {query}");
        codes(None, &query)
            .into_iter()
            .filter(|code| *code == 4012)
            .count()
    };
    // Not projected: the OMIT is a no-op.
    let noop = "SELECT a, b OMIT c FROM t LIMIT 1;";
    assert_eq!(count(noop), 1);
    assert!(
        message_of(None, &format!("DEFINE TABLE t SCHEMALESS; {noop}"), 4012).contains("no effect")
    );
    // Projected and then omitted.
    let contradiction = "SELECT a, b OMIT a FROM t LIMIT 1;";
    assert_eq!(count(contradiction), 1);
    assert!(message_of(
        None,
        &format!("DEFINE TABLE t SCHEMALESS; {contradiction}"),
        4012
    )
    .contains("projected and then omitted"));
    // Two omitted fields, two findings; nested SELECTs are reached.
    assert_eq!(count("SELECT a OMIT b, c FROM t LIMIT 1;"), 2);
    assert_eq!(count("RETURN (SELECT a OMIT b FROM t LIMIT 1);"), 1);
    // Near misses: a wildcard makes OMIT meaningful; no OMIT, nothing to say.
    assert_eq!(count("SELECT * OMIT c FROM t LIMIT 1;"), 0);
    assert_eq!(count("SELECT *, a OMIT c FROM t LIMIT 1;"), 0);
    assert_eq!(count("SELECT a, b FROM t LIMIT 1;"), 0);
}
