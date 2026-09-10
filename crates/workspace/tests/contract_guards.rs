//! One guard per Deny-level code: the smallest input that violates the
//! contract, beside the closest input that does not.
//!
//! The negative corpus pins that a check keeps firing where it fires today; it
//! says nothing about where a check must stay *silent*. A check that fires on
//! the intended form is worse than one that misses the accident, so every code
//! here carries both halves, table-driven: `fires` must raise the code, and
//! `silent` — the same construct spelled correctly — must not.
//!
//! The completeness test at the bottom reads the catalog, so a Deny code added
//! without a row here fails the suite rather than going unguarded.

mod support;

use surrealguard_diagnostics::{catalog, LintLevel};
use surrealguard_syntax::source::SourceId;
use surrealguard_workspace::config::WorkspaceConfig;
use surrealguard_workspace::{analyze_workspace, Workspace, WorkspaceAnalysis};

use support::assert_no_syntax_findings;

/// A self-contained schema every guard runs against: a node table with the
/// field flavours the type contracts need (ASSERT, READONLY, option, array,
/// vector), a linked table, a relation between them, a CHANGEFEED table, a
/// full-text index and an HNSW index, a typed `fn::`, and a `DEFINE PARAM`.
const SCHEMA: &str = "\
DEFINE TABLE user SCHEMAFULL;
DEFINE FIELD name ON user TYPE string;
DEFINE FIELD age ON user TYPE int ASSERT $value >= 0;
DEFINE FIELD email ON user TYPE option<string>;
DEFINE FIELD created ON user TYPE datetime READONLY DEFAULT time::now();
DEFINE FIELD tags ON user TYPE array<string> DEFAULT [];
DEFINE FIELD vec ON user TYPE array<float>;
DEFINE TABLE post SCHEMAFULL;
DEFINE FIELD title ON post TYPE string;
DEFINE FIELD author ON post TYPE record<user>;
DEFINE TABLE wrote SCHEMAFULL TYPE RELATION IN user OUT post;
DEFINE TABLE log SCHEMAFULL CHANGEFEED 1d;
DEFINE FIELD note ON log TYPE string;
DEFINE ANALYZER simple TOKENIZERS blank FILTERS lowercase;
DEFINE INDEX post_title_search ON post FIELDS title FULLTEXT ANALYZER simple BM25;
DEFINE INDEX user_vec ON user FIELDS vec HNSW DIMENSION 3 DIST COSINE;
DEFINE FUNCTION fn::greet($name: string) -> string { RETURN string::concat('hi ', $name); };
DEFINE PARAM $limit VALUE 10;
";

/// One Deny code, its minimal violation, and its nearest valid neighbour.
struct Guard {
    code: u16,
    fires: &'static str,
    silent: &'static str,
}

const fn guard(code: u16, fires: &'static str, silent: &'static str) -> Guard {
    Guard {
        code,
        fires,
        silent,
    }
}

const GUARDS: &[Guard] = &[
    // 1xxx — schema references
    guard(1001, "SELECT name FROM ghost;", "SELECT name FROM user;"),
    guard(1002, "SELECT nope FROM user;", "SELECT name FROM user;"),
    guard(
        1012,
        "REBUILD INDEX ghost ON user;",
        "REBUILD INDEX user_vec ON user;",
    ),
    guard(
        1025,
        "DEFINE FIELD age.part ON user TYPE int;",
        "DEFINE FIELD prefs ON user TYPE object; DEFINE FIELD prefs.theme ON user TYPE string;",
    ),
    guard(
        1027,
        "SELECT name FROM user WHERE name @@ 'x';",
        "SELECT title FROM post WHERE title @@ 'x';",
    ),
    guard(
        1032,
        "DEFINE ANALYZER a1 TOKENIZERS blank FILTERS snowball(klingon);",
        "DEFINE ANALYZER a2 TOKENIZERS blank FILTERS snowball(english);",
    ),
    // 2xxx — types
    guard(
        1033,
        "DEFINE FIELD OVERWRITE id ON user VALUE rand::uuid();",
        "DEFINE FIELD OVERWRITE id ON user DEFAULT rand::uuid();",
    ),
    guard(
        2001,
        "UPDATE user:a SET age = 'x';",
        "UPDATE user:a SET age = 1;",
    ),
    guard(
        2004,
        "SELECT name FROM user WHERE name > 1;",
        "SELECT name FROM user WHERE age > 1;",
    ),
    guard(2007, "RETURN <ghost> 1;", "RETURN <int> 1;"),
    guard(2008, "RETURN <int> 'abc';", "RETURN <int> '42';"),
    guard(
        2012,
        "DEFINE FUNCTION fn::bad() -> string { RETURN 1; };",
        "DEFINE FUNCTION fn::good() -> string { RETURN 'x'; };",
    ),
    // A key the table lacks is 1002 alone; 2017 is a key the table has but
    // this query's rows do not.
    guard(
        2017,
        "SELECT name FROM user ORDER BY age;",
        "SELECT name FROM user ORDER BY name;",
    ),
    guard(
        2018,
        "SELECT name FROM user LIMIT -1;",
        "SELECT name FROM user LIMIT 1;",
    ),
    guard(
        2020,
        "LET $k = 42; KILL $k;",
        "LET $k = u'018e0f3a-1234-7abc-8def-0123456789ab'; KILL $k;",
    ),
    guard(
        2021,
        "SHOW CHANGES FOR TABLE log SINCE 'x';",
        "SHOW CHANGES FOR TABLE log SINCE 1;",
    ),
    guard(
        2022,
        "FOR $x IN 42 { RETURN $x; };",
        "FOR $x IN [1, 2] { RETURN $x; };",
    ),
    guard(
        2025,
        "UPDATE user:a SET created = time::now();",
        "CREATE user SET name = 'a', age = 1, vec = [0.0, 0.0, 0.0], created = time::now();",
    ),
    guard(
        2030,
        "SELECT age[0] FROM user;",
        "SELECT tags[0] FROM user;",
    ),
    guard(2031, "RETURN /unclosed(/;", "RETURN /closed/;"),
    guard(
        2032,
        "RETURN u'not-a-uuid';",
        "RETURN u'018e0f3a-1234-7abc-8def-0123456789ab';",
    ),
    guard(
        2033,
        "UPDATE user:a PATCH [{ op: 'teleport', path: '/age', value: 1 }];",
        "UPDATE user:a PATCH [{ op: 'replace', path: '/age', value: 1 }];",
    ),
    guard(
        2034,
        "CREATE user SET name = 'a';",
        "CREATE user SET name = 'a', age = 1, vec = [0.0, 0.0, 0.0];",
    ),
    guard(
        2035,
        "DEFINE ANALYZER a3 TOKENIZERS blank FILTERS edgengram(5, 2);",
        "DEFINE ANALYZER a4 TOKENIZERS blank FILTERS edgengram(2, 5);",
    ),
    guard(
        2036,
        "RETURN { type: 'Pointt', coordinates: [1, 2] };",
        "RETURN { type: 'Point', coordinates: [1, 2] };",
    ),
    guard(
        2037,
        "DEFINE FIELD grade ON user TYPE int DEFAULT 9 ASSERT $value <= 5;",
        "DEFINE FIELD grade ON user TYPE int DEFAULT 3 ASSERT $value <= 5;",
    ),
    guard(
        2038,
        "UPDATE user:a SET age = -1;",
        "UPDATE user:a SET age = 1;",
    ),
    // 3xxx — graph
    guard(
        3001,
        "SELECT ->post->user FROM user;",
        "SELECT ->wrote->post FROM user;",
    ),
    guard(
        3002,
        "SELECT ->wrote->user FROM post;",
        "SELECT <-wrote<-user FROM post;",
    ),
    guard(
        3009,
        "SELECT age->wrote->post FROM user;",
        "SELECT author->wrote->post FROM post;",
    ),
    // 4xxx — statements
    guard(
        4003,
        "SELECT name FROM ONLY user;",
        "SELECT name FROM ONLY user LIMIT 1;",
    ),
    guard(
        4004,
        "INSERT INTO user (name, age) VALUES ('a');",
        "INSERT INTO user (name, age) VALUES ('a', 1);",
    ),
    guard(4005, "BREAK;", "FOR $x IN [1] { BREAK; };"),
    guard(4007, "COMMIT;", "BEGIN; COMMIT;"),
    guard(
        4009,
        "LIVE SELECT name FROM user:ada;",
        "LIVE SELECT name FROM user;",
    ),
    guard(
        4025,
        "SELECT * FROM user GROUP BY age;",
        "SELECT age, count() FROM user GROUP BY age;",
    ),
    guard(
        4028,
        "SELECT math::sum(age) FROM user;",
        "SELECT math::sum(age) FROM user GROUP ALL;",
    ),
    // 5xxx — functions
    guard(5001, "RETURN ghost::fn(1);", "RETURN string::len('a');"),
    guard(
        5002,
        "RETURN string::len(1, 2);",
        "RETURN string::len('a');",
    ),
    guard(
        5005,
        "SELECT type::field('nope') FROM user;",
        "SELECT type::field('name') FROM user;",
    ),
    // 6xxx — params
    guard(
        6001,
        "SELECT name FROM user WHERE age > $x AND name = $x;",
        "SELECT name FROM user WHERE age > $x AND age < $x;",
    ),
    guard(
        6005,
        "RETURN $before;",
        "DEFINE EVENT touched ON user WHEN $event = 'UPDATE' THEN { RETURN $before; };",
    ),
    guard(6007, "LET $auth = 1;", "LET $mine = 1;"),
];

/// The 8xxx codes gate on `analysis.surrealdb_version`, so each carries the
/// target that lacks the construct and the target that has it.
struct VersionGuard {
    code: u16,
    query: &'static str,
    fires_on: &'static str,
    silent_on: &'static str,
}

const VERSION_GUARDS: &[VersionGuard] = &[
    VersionGuard {
        code: 8001,
        query: "RETURN set::len([1, 2]);",
        fires_on: "2.2",
        silent_on: "3.0",
    },
    VersionGuard {
        code: 8002,
        query: "RETURN 'a' ~ 'b';",
        fires_on: "3.0",
        silent_on: "2.0",
    },
    VersionGuard {
        code: 8003,
        query: "UPSERT user:a SET age = 1;",
        fires_on: "1.5",
        silent_on: "2.0",
    },
];

/// Deny codes no `.surql` input can reach: the grammar rejects the violation
/// before analysis (`TIMEOUT 5` / `TIMEOUT $t` are parse errors), so the
/// contract is parser-covered and has no guard row.
const PARSER_COVERED: &[u16] = &[2019];

/// The query's analysis: every finding, and the query source's own id, so a
/// guard can count the query's findings alone. The schema is analyzed beside
/// it and has findings of its own under an old target (its `FULLTEXT
/// ANALYZER` and `HNSW` are 3.0 syntax) that must not be mistaken for the
/// query's.
struct Analyzed {
    output: WorkspaceAnalysis,
    query: SourceId,
}

impl Analyzed {
    /// How many findings with catalog number `number` landed on the query.
    fn codes(&self, number: u16) -> usize {
        self.output
            .diagnostics
            .iter()
            .filter(|finding| finding.span().source() == &self.query)
            .filter(|finding| finding.code().number() == number)
            .count()
    }

    /// `(code, message)` of every finding on the query, for assertion text.
    fn findings(&self) -> Vec<(String, String)> {
        self.output
            .diagnostics
            .iter()
            .filter(|finding| finding.span().source() == &self.query)
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect()
    }
}

fn analyze(version: Option<&str>, query: &str) -> Analyzed {
    let config = version.map_or_else(WorkspaceConfig::default, |version| {
        WorkspaceConfig::from_toml_str(&format!("[analysis]\nsurrealdb_version = \"{version}\"\n"))
            .expect("config parses")
    });
    let mut workspace = Workspace::new(config);
    workspace.add_virtual_source("schema".into(), SCHEMA.into());
    let query = workspace.add_virtual_source("query".into(), query.into());
    let output = analyze_workspace(&workspace);
    assert_no_syntax_findings(&output.diagnostics);
    Analyzed { output, query }
}

#[test]
fn every_deny_code_fires_on_its_minimal_violation() {
    for guard in GUARDS {
        let analyzed = analyze(None, guard.fires);
        assert!(
            analyzed.codes(guard.code) >= 1,
            "{} did not fire on `{}`; got {:?}",
            guard.code,
            guard.fires,
            analyzed.findings()
        );
    }
    for guard in VERSION_GUARDS {
        let analyzed = analyze(Some(guard.fires_on), guard.query);
        assert!(
            analyzed.codes(guard.code) >= 1,
            "{} did not fire on `{}` targeting {}; got {:?}",
            guard.code,
            guard.query,
            guard.fires_on,
            analyzed.findings()
        );
    }
}

#[test]
fn every_deny_code_stays_silent_on_its_nearest_valid_neighbour() {
    for guard in GUARDS {
        let analyzed = analyze(None, guard.silent);
        assert_eq!(
            analyzed.codes(guard.code),
            0,
            "{} fired on the valid near-miss `{}`: {:?}",
            guard.code,
            guard.silent,
            analyzed.findings()
        );
    }
    for guard in VERSION_GUARDS {
        let analyzed = analyze(Some(guard.silent_on), guard.query);
        assert_eq!(
            analyzed.codes(guard.code),
            0,
            "{} fired on `{}` targeting {}, which has the construct",
            guard.code,
            guard.query,
            guard.silent_on
        );
        let unconfigured = analyze(None, guard.query);
        assert_eq!(
            unconfigured.codes(guard.code),
            0,
            "{} fired on `{}` with no target configured",
            guard.code,
            guard.query
        );
    }
}

#[test]
fn every_deny_code_in_the_catalog_has_a_guard() {
    let guarded: Vec<u16> = GUARDS
        .iter()
        .map(|guard| guard.code)
        .chain(VERSION_GUARDS.iter().map(|guard| guard.code))
        .chain(PARSER_COVERED.iter().copied())
        .collect();
    let unguarded: Vec<u16> = catalog::all()
        .filter(|entry| entry.default_level == LintLevel::Deny)
        .map(|entry| entry.number)
        .filter(|number| !guarded.contains(number))
        .collect();
    assert!(
        unguarded.is_empty(),
        "Deny-level codes with no fire/silent guard pair: {unguarded:?}"
    );
    for guard in GUARDS {
        assert_eq!(
            catalog::entry(guard.code).map(|entry| entry.default_level),
            Some(LintLevel::Deny),
            "{} is guarded here but is not a Deny-level code",
            guard.code
        );
    }
}
