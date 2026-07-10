//! The diagnostic code registry, generated from and consistency-tested
//! against `docs/plans/2026-07-07-diagnostic-catalog.md` — the canonical
//! catalog. Emission sites construct findings through [`finding`] so a
//! code's intrinsic severity can never drift from the catalog.

use surrealguard_syntax::span::SourceSpan;

use crate::{Finding, FindingCode, Severity};

/// One registered diagnostic: its number, short label, and intrinsic
/// severity class. Message text is composed at the emission site; the
/// label names the *kind* of problem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogEntry {
    pub number: u16,
    pub label: &'static str,
    pub severity: Severity,
}

/// Every code in the catalog, ordered by number. Append-only within each
/// thousand-block family.
#[rustfmt::skip]
const ENTRIES: &[(u16, &str, Severity)] = &[
    (1001, "a table reference names a known table", Severity::Error),
    (1002, "a field reference names a declared field of its row's table (schemafull only; FLEXIBLE subtrees exempt)", Severity::Error),
    (1012, "a schema-object reference names a known object of that kind", Severity::Error),
    (1021, "REMOVE removes something that exists", Severity::Warning),
    (1022, "a definition does not silently redefine (OVERWRITE states intent)", Severity::Warning),
    (1023, "FETCH names something that can hold records", Severity::Error),
    (1024, "SPLIT names a collection field", Severity::Error),
    (1025, "a subfield is declared under an object-shaped parent", Severity::Error),
    (1027, "an index-backed operator has its supporting index", Severity::Error),
    (1029, "each index covers a distinct field set", Severity::Warning),
    (1032, "DEFINE ANALYZER components name known tokenizers/filters/languages", Severity::Error),
    (2001, "a value written to a field inhabits the field's declared type", Severity::Error),
    (2004, "the operands make sense together for the operator", Severity::Error),
    (2005, "a condition position expects a boolean", Severity::Warning),
    (2007, "a cast names a known type", Severity::Error),
    (2008, "a conversion can succeed", Severity::Error),
    (2012, "a body returns what it declares", Severity::Error),
    (2015, "a value-requiring position gets a value that is always present", Severity::Warning),
    (2017, "ORDER BY keys name fields available on the result rows (or RAND())", Severity::Error),
    (2018, "LIMIT/START take a non-negative integer", Severity::Error),
    (2019, "TIMEOUT takes a duration", Severity::Error),
    (2020, "KILL takes a live-query uuid", Severity::Error),
    (2021, "SHOW SINCE takes a versionstamp or datetime", Severity::Error),
    (2022, "FOR iterates something iterable", Severity::Error),
    (2025, "READONLY fields are written only at creation", Severity::Error),
    (2026, "computed (VALUE-clause) fields are not hand-assigned", Severity::Warning),
    (2030, "index/filter/splat apply to collections", Severity::Error),
    (2031, "a regex literal compiles", Severity::Error),
    (2032, "literal content is valid for its kind", Severity::Error),
    (2033, "PATCH operations are well-formed", Severity::Error),
    (2034, "required fields are provided at creation", Severity::Error),
    (2035, "DEFINE ANALYZER filter arguments are valid", Severity::Error),
    (2036, "GeoJSON literals have their declared shape", Severity::Error),
    (3001, "a step traverses a relation table", Severity::Error),
    (3002, "the usage matches the relation's declared shape (`in`->edge->`out`)", Severity::Error),
    (3004, "a FROM-position chain is complete (edge->target pairs)", Severity::Error),
    (3009, "a traversal starts from records", Severity::Error),
    (3011, "graph recursion is bounded", Severity::Warning),
    (4001, "clause not valid on this statement", Severity::Error),
    (4002, "SELECT VALUE with multiple projections", Severity::Error),
    (4003, "ONLY on a table-wide target without LIMIT 1", Severity::Error),
    (4004, "INSERT tuple column/value count mismatch", Severity::Error),
    (4005, "BREAK/CONTINUE outside a loop", Severity::Error),
    (4006, "unreachable statements after RETURN/BREAK/THROW", Severity::Warning),
    (4007, "transaction pairing contract: BEGIN opens exactly one transaction that COMMIT/CANCEL closes", Severity::Error),
    (4009, "LIVE SELECT with unsupported clause", Severity::Error),
    (4010, "duplicate SET target in one statement", Severity::Warning),
    (4011, "duplicate projection key/alias", Severity::Warning),
    (4012, "OMIT without a wildcard projection", Severity::Warning),
    (4013, "GROUP BY field not in projections", Severity::Warning),
    (4016, "empty block", Severity::Hint),
    (4017, "block ends with LET — its value is NONE", Severity::Warning),
    (4018, "side-effecting subquery in read position", Severity::Warning),
    (4019, "CREATE/INSERT on a relation table without `in`/`out`", Severity::Warning),
    (4020, "RETURN mode meaningless for the statement", Severity::Warning),
    (4021, "SHOW CHANGES on a table without CHANGEFEED", Severity::Error),
    (4022, "SELECT from a DROP table", Severity::Warning),
    (5001, "a call resolves to a function that exists", Severity::Error),
    (5002, "a call matches the function's signature", Severity::Error),
    (5005, "a const argument satisfies the function's value contract", Severity::Error),
    (5009, "`fn::` definitions terminate (no direct/mutual recursion cycles)", Severity::Warning),
    (5010, "events do not trigger themselves (directly or in a cycle)", Severity::Warning),
    (6001, "conflicting constraints on one param", Severity::Error),
    (6002, "param shadows a DEFINE PARAM with a different kind", Severity::Warning),
    (6003, "unresolvable dynamic construct (analyzer limitation)", Severity::Hint),
    (6004, "param used before its LET in source order", Severity::Warning),
    (6005, "context param used outside its context", Severity::Error),
    (6006, "host-declared type contradicts query constraint", Severity::Error),
    (6007, "assignment to a protected parameter", Severity::Error),
    (7001, "unused LET binding", Severity::Warning),
    (7002, "LET shadowing", Severity::Hint),
    (7003, "mixed-kind array literal", Severity::Hint),
    (7004, "control flow is decided by a constant", Severity::Warning),
    (
        7005,
        "a comparison against a closed literal set must be able to match",
        Severity::Warning,
    ),
    (7006, "empty IN/CONTAINS list", Severity::Warning),
    (7007, "SELECT * with explicit fields", Severity::Hint),
    (7008, "schemaless table in a typed workspace", Severity::Hint),
    (7009, "whole-table UPDATE/DELETE without WHERE", Severity::Warning),
    (7011, "assignment to `id` in SET", Severity::Warning),
    (7012, "blocking or side-effecting call in a computed context", Severity::Warning),
    (
        7013,
        "a suppression directive names a catalog code (with a reason when required)",
        Severity::Warning,
    ),
    (8001, "every function used exists in the configured target version", Severity::Error),
    (8003, "syntax requires a newer version", Severity::Error),
];

/// Looks up a catalog entry by code number.
pub fn entry(number: u16) -> Option<CatalogEntry> {
    let index = ENTRIES.binary_search_by_key(&number, |(n, _, _)| *n).ok()?;
    let (number, label, severity) = ENTRIES[index];
    Some(CatalogEntry {
        number,
        label,
        severity,
    })
}

/// Constructs a finding for a cataloged code: the severity comes from the
/// registry, never from the caller.
///
/// # Panics
///
/// Panics if `number` is not in the catalog — emitting an unregistered
/// code is a bug in the analyzer, not an input condition.
pub fn finding(span: SourceSpan, number: u16, message: impl Into<String>) -> Finding {
    let entry =
        entry(number).unwrap_or_else(|| panic!("diagnostic code {number} is not in the catalog"));
    Finding::new(
        span,
        FindingCode::from_number(number),
        entry.severity,
        message,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn entries_are_sorted_and_unique() {
        for pair in ENTRIES.windows(2) {
            assert!(pair[0].0 < pair[1].0, "{} then {}", pair[0].0, pair[1].0);
        }
    }

    #[test]
    fn registry_matches_the_catalog_document() {
        let doc_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/plans/2026-07-07-diagnostic-catalog.md");
        let doc = std::fs::read_to_string(&doc_path).expect("catalog document exists");

        let mut documented = Vec::new();
        for line in doc.lines() {
            // Escaped pipes (`\|`) appear inside example cells; neutralize
            // them before splitting into columns.
            let neutral = line.replace("\\|", "\u{0}");
            let cells: Vec<&str> = neutral
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            let Some(number) = cells.first().and_then(|c| c.parse::<u16>().ok()) else {
                continue;
            };
            let severity_cell = cells
                .get(3)
                .and_then(|c| c.split_whitespace().next())
                .unwrap_or("");
            let severity = match severity_cell {
                "E" => Severity::Error,
                "W" => Severity::Warning,
                "I" => Severity::Hint,
                other => panic!("code {number}: unparsable severity {other:?}"),
            };
            documented.push((number, severity));
        }
        documented.sort();

        let registered: Vec<(u16, Severity)> = ENTRIES.iter().map(|(n, _, s)| (*n, *s)).collect();
        assert_eq!(
            documented, registered,
            "the catalog document and the code registry disagree"
        );
    }

    #[test]
    fn finding_takes_severity_from_the_registry() {
        use surrealguard_syntax::source::SourceId;
        use surrealguard_syntax::span::ByteRange;

        let span = SourceSpan::new(
            SourceId::new("test"),
            ByteRange::new(0, 1).expect("valid range"),
        );
        let finding = finding(span, 4010, "duplicate SET target");
        assert_eq!(finding.severity(), Severity::Warning);
        assert_eq!(finding.code().number(), 4010);
    }
}
