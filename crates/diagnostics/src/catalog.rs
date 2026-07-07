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
    (1001, "unknown table in FROM/target", Severity::Error),
    (1002, "unknown field in projection", Severity::Error),
    (1003, "unknown field in WHERE/expression", Severity::Error),
    (1004, "unknown field as SET/UNSET target", Severity::Error),
    (1005, "unknown field as CONTENT/MERGE key", Severity::Error),
    (1006, "unknown field in RETURN projection", Severity::Error),
    (1007, "unknown field in OMIT", Severity::Error),
    (1008, "unknown field in FETCH", Severity::Error),
    (1009, "unknown field in SPLIT", Severity::Error),
    (1010, "unknown field in GROUP/ORDER BY", Severity::Error),
    (1011, "unknown INSERT tuple column", Severity::Error),
    (1012, "unknown index target", Severity::Error),
    (1013, "unknown event target", Severity::Error),
    (1014, "unknown analyzer reference", Severity::Error),
    (1015, "unknown user function", Severity::Error),
    (1016, "`record<t>` names unknown table", Severity::Error),
    (1017, "relation endpoint names unknown table", Severity::Error),
    (1018, "DEFINE FIELD/INDEX/EVENT on unknown table", Severity::Error),
    (1019, "index field not on table", Severity::Error),
    (1020, "event WHEN/THEN references unknown field", Severity::Error),
    (1021, "REMOVE of object that doesn't exist", Severity::Warning),
    (1022, "duplicate definition without OVERWRITE", Severity::Warning),
    (1023, "FETCH of a non-record field", Severity::Error),
    (1024, "SPLIT of a non-collection field", Severity::Error),
    (1025, "subfield declared under a non-object field", Severity::Error),
    (1026, "use of a table/field after its REMOVE earlier in the script", Severity::Error),
    (1027, "full-text operator/function without a search index", Severity::Error),
    (1028, "KNN operator without a vector index", Severity::Error),
    (1029, "duplicate index over identical fields", Severity::Warning),
    (1030, "WITH index hint names unknown index", Severity::Error),
    (1031, "PATCH path names unknown field", Severity::Error),
    (1032, "unknown tokenizer/filter/snowball language in DEFINE ANALYZER", Severity::Error),
    (2001, "SET value not assignable to field", Severity::Error),
    (2002, "CONTENT/MERGE value not assignable per key", Severity::Error),
    (2003, "INSERT tuple value not assignable to column", Severity::Error),
    (2004, "operand kinds incompatible for operator", Severity::Error),
    (2005, "IF condition not boolean", Severity::Warning),
    (2006, "WHERE clause not boolean-ish", Severity::Hint),
    (2007, "cast to unknown type", Severity::Error),
    (2008, "cast that cannot succeed", Severity::Warning),
    (2009, "DEFAULT not assignable to declared type", Severity::Error),
    (2010, "VALUE clause not assignable to declared type", Severity::Error),
    (2011, "ASSERT not boolean", Severity::Error),
    (2012, "fn:: body return doesn't match declared `->` type", Severity::Error),
    (2013, "closure body vs declared return mismatch", Severity::Error),
    (2014, "negation of non-numeric", Severity::Error),
    (2015, "possibly-NONE value where value required", Severity::Warning),
    (2016, "assigning NONE to non-optional field", Severity::Error),
    (2017, "ORDER BY on non-comparable kind", Severity::Warning),
    (2018, "LIMIT/START not an integer", Severity::Error),
    (2019, "TIMEOUT not a duration", Severity::Error),
    (2020, "KILL argument not a uuid", Severity::Error),
    (2021, "SHOW SINCE not versionstamp/datetime", Severity::Error),
    (2022, "FOR over a non-iterable", Severity::Error),
    (2023, "const conversion provably fails", Severity::Error),
    (2024, "negative const LIMIT/START", Severity::Error),
    (2025, "assignment to a READONLY field", Severity::Error),
    (2026, "assignment to a computed (VALUE-clause) field", Severity::Warning),
    (2027, "record link targets the wrong table", Severity::Error),
    (2028, "VERSION clause not a datetime", Severity::Error),
    (2029, "compound assignment incompatible with field kind", Severity::Error),
    (2030, "index/filter/splat on a non-collection", Severity::Error),
    (2031, "invalid const regex pattern", Severity::Error),
    (2032, "invalid literal content", Severity::Error),
    (2033, "invalid PATCH operation", Severity::Error),
    (2034, "missing required field on CREATE/CONTENT/INSERT", Severity::Error),
    (2035, "invalid analyzer filter arguments", Severity::Error),
    (2036, "invalid GeoJSON literal shape", Severity::Error),
    (3001, "traversal edge is not a relation table", Severity::Error),
    (3002, "relation does not connect these tables in this direction", Severity::Error),
    (3003, "traversal target unreachable from edge", Severity::Error),
    (3004, "dangling edge hop (edge without target where one is required)", Severity::Error),
    (3005, "multi-target step cannot resolve", Severity::Warning),
    (3006, "RELATE endpoints violate the relation's IN/OUT", Severity::Error),
    (3007, "RELATE edge is not a relation table", Severity::Error),
    (3008, "RELATE endpoint table unknown", Severity::Error),
    (3009, "graph step off a non-record position", Severity::Error),
    (3010, "FETCH alias that is not a record target", Severity::Warning),
    (3011, "unbounded graph recursion", Severity::Warning),
    (4001, "clause not valid on this statement", Severity::Error),
    (4002, "SELECT VALUE with multiple projections", Severity::Error),
    (4003, "ONLY on a multi-row target", Severity::Warning),
    (4004, "INSERT tuple column/value count mismatch", Severity::Error),
    (4005, "BREAK/CONTINUE outside a loop", Severity::Error),
    (4006, "unreachable statements after RETURN/BREAK/THROW", Severity::Warning),
    (4007, "COMMIT/CANCEL without BEGIN", Severity::Error),
    (4008, "nested BEGIN", Severity::Error),
    (4009, "LIVE SELECT with unsupported clause", Severity::Error),
    (4010, "duplicate SET target in one statement", Severity::Warning),
    (4011, "duplicate projection key/alias", Severity::Warning),
    (4012, "OMIT without a wildcard projection", Severity::Warning),
    (4013, "GROUP BY field not in projections", Severity::Warning),
    (4014, "RETURN outside a function/block context where invalid", Severity::Warning),
    (4015, "BEGIN never closed", Severity::Error),
    (4016, "empty block", Severity::Hint),
    (4017, "block ends with LET — its value is NONE", Severity::Warning),
    (4018, "side-effecting subquery in read position", Severity::Warning),
    (4019, "CREATE/INSERT on a relation table without `in`/`out`", Severity::Warning),
    (4020, "RETURN mode meaningless for the statement", Severity::Warning),
    (4021, "SHOW CHANGES on a table without CHANGEFEED", Severity::Error),
    (4022, "SELECT from a DROP table", Severity::Warning),
    (5001, "unknown function", Severity::Error),
    (5002, "wrong argument count", Severity::Error),
    (5003, "argument kind mismatch (per-argument span)", Severity::Error),
    (5004, "closure parameter count wrong for consumer", Severity::Warning),
    (5005, "const value-dependent violation", Severity::Error),
    (5006, "fn:: argument count/kind vs its DEFINE", Severity::Error),
    (5007, "non-string idiom in value-dependent position", Severity::Error),
    (5008, "method not available on receiver kind", Severity::Error),
    (5009, "fn:: recursion cycle (direct or mutual)", Severity::Warning),
    (5010, "event trigger cycle", Severity::Warning),
    (5011, "const table argument names unknown table", Severity::Error),
    (5012, "const argument outside the function's valid range", Severity::Warning),
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
    (7004, "condition is constant", Severity::Warning),
    (7005, "comparison always false by kind", Severity::Warning),
    (7006, "empty IN/CONTAINS list", Severity::Warning),
    (7007, "SELECT * with explicit fields", Severity::Hint),
    (7008, "schemaless table in a typed workspace", Severity::Hint),
    (7009, "whole-table UPDATE/DELETE without WHERE", Severity::Warning),
    (7010, "FOR over an empty const collection", Severity::Warning),
    (7011, "assignment to `id` in SET", Severity::Warning),
    (7012, "blocking or side-effecting call in a computed context", Severity::Warning),
    (8001, "function not available in the configured version", Severity::Error),
    (8002, "function renamed in the configured version", Severity::Error),
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
        let finding = finding(span, 4003, "ONLY with a multi-row target");
        assert_eq!(finding.severity(), Severity::Warning);
        assert_eq!(finding.code().number(), 4003);
    }
}
