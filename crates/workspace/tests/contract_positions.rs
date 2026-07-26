//! Every position that requires a kind, quantified over.
//!
//! `DEFINE FIELD e ON t TYPE 'red' | 'blue' VALUE 'green'` shipped silent while
//! `CREATE t SET e = 'green'` reported, and 917 green tests could not have
//! caught it: the invariant *was* tested, four times, at one site. A list of
//! `DEFINE FIELD` queries existentially quantified over one position says
//! nothing about the other twenty-two that implement the same rule.
//!
//! So this file quantifies the other way. [`Position`] is production code;
//! `SITES` gives each position one query template, `CASES` gives each contract
//! a value that inhabits it, one that provably does not, and one that cannot be
//! proven either way — and three properties are asserted at every crossing.
//!
//! The third property is the one that matters most, and the one a point-fix
//! forgets: an error-severity finding aborts code generation for a whole
//! workspace, so a check that fires on `$p` is worse than a check that misses
//! `'green'`. Closing a gap is only safe once acceptance is verified at every
//! position, not just at the one being fixed.
//!
//! `KNOWN_GAPS` is the debt list, in `any_baseline.txt`'s shape: it may shrink
//! and may not grow. Fixing a position without deleting its line fails
//! `known_gaps_are_still_gaps`, so it cannot go stale in either direction.

use surrealguard_workspace::analyzer::contract::Position;
use surrealguard_workspace::{analyze_workspace, Workspace};

/// One invariant: a declared kind, a value that inhabits it, one that provably
/// does not, and one that cannot be proven either way.
struct Case {
    /// Fills the `{ty}` hole. `any` for positions whose contract is fixed by
    /// the syntax rather than declared (`LIMIT`, a condition).
    declared: &'static str,
    /// A value the position must accept.
    ok: &'static str,
    /// A value the position must reject. Empty when the grammar admits no
    /// wrong value at all in this position — see `TIMEOUT`.
    bad: &'static str,
    /// A value the position must stay silent about.
    unprovable: &'static str,
}

/// One position, as a schema/query template pair with `{ty}` and `{val}` holes.
struct Site {
    position: Position,
    /// The finding this position raises when its contract is violated.
    code: u16,
    /// Appended to [`PRELUDE`]; analyzed as schema.
    schema: &'static str,
    /// Analyzed as a query against that schema.
    query: &'static str,
    /// Which invariants apply here.
    cases: &'static [Case],
}

/// The tables every site shares. `t` carries the field under test; `s` carries
/// the fixed shapes the clause positions name.
const PRELUDE: &str = "\
DEFINE TABLE user SCHEMAFULL;
DEFINE FIELD name ON user TYPE string;
DEFINE TABLE team SCHEMAFULL;
DEFINE TABLE t SCHEMAFULL;
DEFINE FIELD f ON t TYPE {ty};
DEFINE TABLE s SCHEMAFULL;
DEFINE FIELD tags ON s TYPE array<string>;
DEFINE FIELD link ON s TYPE record<user>;
DEFINE FIELD label ON s TYPE string;
DEFINE FIELD opaque ON s TYPE any;
";

/// The five contracts every *declared*-kind position is quantified over.
const DECLARED: &[Case] = &[
    Case {
        declared: "'red' | 'blue'",
        ok: "'red'",
        bad: "'green'",
        unprovable: "$p",
    },
    Case {
        declared: "int",
        ok: "1",
        bad: "'x'",
        unprovable: "$p",
    },
    Case {
        declared: "option<int>",
        ok: "NONE",
        bad: "'x'",
        unprovable: "$p",
    },
    Case {
        declared: "record<user>",
        ok: "user:1",
        bad: "team:1",
        unprovable: "$p",
    },
    Case {
        declared: "array<int>",
        ok: "[1]",
        bad: "['x']",
        unprovable: "$p",
    },
];

/// A condition position: the contract is `bool`, whatever is declared.
const CONDITION: &[Case] = &[Case {
    declared: "any",
    ok: "true",
    bad: "'notabool'",
    unprovable: "$p",
}];

/// An integer clause (`LIMIT`, `START`). The value reaches the clause through
/// a `LET`, because the grammar admits only a number or a param there — a
/// literal `LIMIT 'x'` is a syntax error, not a contract violation.
const INTEGER: &[Case] = &[Case {
    declared: "any",
    ok: "1",
    bad: "'x'",
    unprovable: "$p",
}];

/// A duration clause (`TIMEOUT`). `bad` is empty on purpose: the grammar is
/// `seq(TIMEOUT, Duration)`, so the clause cannot hold anything but a duration
/// literal and its 2019 check is unreachable from any input.
const DURATION: &[Case] = &[Case {
    declared: "any",
    ok: "1s",
    bad: "",
    unprovable: "1s",
}];

/// A clause that names a field of `s` rather than writing a value.
const FIELD_SHAPE_COLLECTION: &[Case] = &[Case {
    declared: "any",
    ok: "tags",
    bad: "label",
    unprovable: "opaque",
}];

/// `FETCH` needs something that holds records.
const FIELD_SHAPE_RECORD: &[Case] = &[Case {
    declared: "any",
    ok: "link",
    bad: "label",
    unprovable: "opaque",
}];

/// A collection to iterate, reached through a `LET` for the same reason
/// `LIMIT` is.
const ITERABLE: &[Case] = &[Case {
    declared: "any",
    ok: "[1, 2]",
    bad: "'x'",
    unprovable: "$p",
}];

/// A cast operand, whose contract is "this conversion can succeed".
const CASTABLE: &[Case] = &[Case {
    declared: "any",
    ok: "'1'",
    bad: "'abc'",
    unprovable: "$p",
}];

const SITES: &[Site] = &[
    Site {
        position: Position::MutationSet,
        code: 2001,
        schema: "",
        query: "CREATE t SET f = {val};",
        cases: DECLARED,
    },
    Site {
        position: Position::MutationContent,
        code: 2001,
        schema: "",
        query: "CREATE t CONTENT { f: {val} };",
        cases: DECLARED,
    },
    Site {
        position: Position::MutationMerge,
        code: 2001,
        schema: "",
        query: "UPDATE t MERGE { f: {val} };",
        cases: DECLARED,
    },
    Site {
        position: Position::InsertValues,
        code: 2001,
        schema: "",
        query: "INSERT INTO t (f) VALUES ({val});",
        cases: DECLARED,
    },
    Site {
        position: Position::FieldValue,
        code: 2001,
        schema: "DEFINE FIELD g ON t TYPE {ty} VALUE {val};",
        query: "RETURN 1;",
        cases: DECLARED,
    },
    Site {
        position: Position::FieldDefault,
        code: 2001,
        schema: "DEFINE FIELD g ON t TYPE {ty} DEFAULT {val};",
        query: "RETURN 1;",
        cases: DECLARED,
    },
    Site {
        position: Position::FieldComputed,
        code: 2001,
        schema: "DEFINE FIELD g ON t TYPE {ty} COMPUTED {val};",
        query: "RETURN 1;",
        cases: DECLARED,
    },
    Site {
        position: Position::FieldAssert,
        code: 2005,
        schema: "DEFINE FIELD g ON t TYPE any ASSERT {val};",
        query: "RETURN 1;",
        cases: CONDITION,
    },
    Site {
        position: Position::FunctionArg,
        code: 5002,
        schema: "DEFINE FUNCTION fn::take($a: {ty}) { RETURN $a; };",
        query: "RETURN fn::take({val});",
        cases: DECLARED,
    },
    Site {
        position: Position::FunctionReturn,
        code: 2012,
        schema: "DEFINE FUNCTION fn::give() -> {ty} { RETURN {val}; };",
        query: "RETURN 1;",
        cases: DECLARED,
    },
    Site {
        position: Position::ParamDefault,
        code: 2001,
        schema: "DEFINE PARAM $q VALUE {val};",
        query: "RETURN 1;",
        cases: DECLARED,
    },
    Site {
        position: Position::Limit,
        code: 2018,
        schema: "",
        query: "LET $n = {val};\nSELECT * FROM s LIMIT $n;",
        cases: INTEGER,
    },
    Site {
        position: Position::Start,
        code: 2018,
        schema: "",
        query: "LET $n = {val};\nSELECT * FROM s START $n;",
        cases: INTEGER,
    },
    Site {
        position: Position::Timeout,
        code: 2019,
        schema: "",
        query: "SELECT * FROM s TIMEOUT {val};",
        cases: DURATION,
    },
    Site {
        position: Position::Split,
        code: 1024,
        schema: "",
        query: "SELECT * FROM s SPLIT {val};",
        cases: FIELD_SHAPE_COLLECTION,
    },
    Site {
        position: Position::Fetch,
        code: 1023,
        schema: "",
        query: "SELECT * FROM s FETCH {val};",
        cases: FIELD_SHAPE_RECORD,
    },
    Site {
        position: Position::ForIterable,
        code: 2022,
        schema: "",
        query: "LET $c = {val};\nFOR $i IN $c { RETURN $i; };",
        cases: ITERABLE,
    },
    Site {
        position: Position::Cast,
        code: 2008,
        schema: "",
        query: "RETURN <int> {val};",
        cases: CASTABLE,
    },
    Site {
        position: Position::WhereSelect,
        code: 2005,
        schema: "",
        query: "SELECT * FROM s WHERE {val};",
        cases: CONDITION,
    },
    Site {
        position: Position::WhereMutation,
        code: 2005,
        schema: "",
        query: "UPDATE s SET label = 'a' WHERE {val};",
        cases: CONDITION,
    },
    Site {
        position: Position::IfCond,
        code: 2005,
        schema: "",
        query: "IF {val} { RETURN 1; };",
        cases: CONDITION,
    },
    Site {
        position: Position::EventWhen,
        code: 2005,
        schema: "DEFINE EVENT ev ON s WHEN {val} THEN { RETURN 1; };",
        query: "RETURN 1;",
        cases: CONDITION,
    },
    Site {
        position: Position::PermissionPredicate,
        code: 2005,
        schema: "DEFINE TABLE p SCHEMAFULL PERMISSIONS FOR select WHERE {val};",
        query: "RETURN 1;",
        cases: CONDITION,
    },
];

/// The positions that do not honour a contract yet, as `(position, declared)`
/// with `"*"` meaning every case. **This list may only shrink.** A line here is
/// a committed, countable hole rather than a paragraph in a design document.
const KNOWN_GAPS: &[(Position, &str)] = &[
    (Position::FieldComputed, "*"), // no contract at all: absent from the clause loop
    (Position::ParamDefault, "*"),  // no declared type to check against
];

/// Whether the gap list excuses this crossing.
fn is_known_gap(position: Position, declared: &str) -> bool {
    KNOWN_GAPS
        .iter()
        .any(|(gap, ty)| *gap == position && (*ty == "*" || *ty == declared))
}

/// Fills a template's `{ty}` and `{val}` holes.
fn fill(template: &str, declared: &str, value: &str) -> String {
    template.replace("{ty}", declared).replace("{val}", value)
}

/// Whether the site's code fires for `value` at this case.
fn fires(site: &Site, case: &Case, value: &str) -> bool {
    let mut workspace = Workspace::default();
    let schema = format!(
        "{}{}",
        fill(PRELUDE, case.declared, value),
        fill(site.schema, case.declared, value)
    );
    workspace.add_virtual_source("schema".into(), schema);
    workspace.add_virtual_source("query".into(), fill(site.query, case.declared, value));
    analyze_workspace(&workspace)
        .diagnostics
        .iter()
        .any(|finding| finding.code().number() == site.code)
}

/// Runs `property` at every (site, case) crossing and reports the whole map at
/// once — a wall of rows beats a single `assert_eq` when twenty-three positions
/// implement one rule.
fn every_crossing(what: &str, property: impl Fn(&Site, &Case) -> Result<(), String>) {
    let mut failures = Vec::new();
    for site in SITES {
        for case in site.cases {
            if let Err(detail) = property(site, case) {
                failures.push(format!(
                    "  ✗ {:<20} {:<16} {detail}",
                    site.position.name(),
                    case.declared
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{what} — {} position(s) do not:\n\n{}\n",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn every_position_is_covered_by_a_site() {
    let missing: Vec<&str> = Position::ALL
        .iter()
        .filter(|position| !SITES.iter().any(|site| site.position == **position))
        .map(|position| position.name())
        .collect();
    assert!(
        missing.is_empty(),
        "a position with no site is a position nothing tests: {missing:?}"
    );
}

#[test]
fn every_position_rejects_a_provably_wrong_value() {
    every_crossing("a provably wrong value must be reported", |site, case| {
        if case.bad.is_empty() || is_known_gap(site.position, case.declared) {
            return Ok(());
        }
        if fires(site, case, case.bad) {
            return Ok(());
        }
        Err(format!(
            "no E{:04} for `{}`",
            site.code,
            fill(site.query, case.declared, case.bad).trim()
        ))
    });
}

#[test]
fn every_position_accepts_a_valid_value() {
    every_crossing("a value that inhabits the contract must be silent", |site, case| {
        if fires(site, case, case.ok) {
            return Err(format!(
                "false E{:04} for `{}`",
                site.code,
                fill(site.query, case.declared, case.ok).trim()
            ));
        }
        Ok(())
    });
}

#[test]
fn every_position_stays_silent_on_an_unprovable_one() {
    // The acceptance half. A check that fires here is worse than one that
    // misses a `bad`: an error aborts `generate` for the whole workspace.
    every_crossing("an unprovable value must be silent", |site, case| {
        if fires(site, case, case.unprovable) {
            return Err(format!(
                "false E{:04} for `{}`",
                site.code,
                fill(site.query, case.declared, case.unprovable).trim()
            ));
        }
        Ok(())
    });
}

#[test]
fn known_gaps_are_still_gaps() {
    // The ratchet. Closing a position without deleting its line fails here,
    // so the debt list cannot go stale in the direction that flatters us.
    let mut fixed = Vec::new();
    for site in SITES {
        for case in site.cases {
            if case.bad.is_empty() {
                continue;
            }
            if is_known_gap(site.position, case.declared) && fires(site, case, case.bad) {
                fixed.push(format!(
                    "  ({}, {:?}) — delete this line",
                    site.position.name(),
                    case.declared
                ));
            }
        }
    }
    assert!(
        fixed.is_empty(),
        "these gaps are closed; KNOWN_GAPS may only shrink:\n\n{}\n",
        fixed.join("\n")
    );
}
