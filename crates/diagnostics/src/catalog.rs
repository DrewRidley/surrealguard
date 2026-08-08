//! The diagnostic code registry, generated from and consistency-tested
//! against `docs/plans/2026-07-07-diagnostic-catalog.md` — the canonical
//! catalog. Emission sites construct findings through [`finding`] so a
//! code's intrinsic severity can never drift from the catalog.

use surrealguard_syntax::span::SourceSpan;

use crate::{Finding, FindingCode, LintLevel, Severity};

/// One registered diagnostic: its number, short label, intrinsic severity
/// class, and default lint level. Message text is composed at the emission
/// site; the label names the *kind* of problem. The `default_level` is the
/// rustc-style policy default a consumer applies when a `[lints]` override
/// does not name the code: `allow` suppresses it, `warn` reports a warning,
/// `deny` promotes it to an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogEntry {
    /// The code number within its thousand-block family.
    pub number: u16,
    /// A short phrase naming the contract this code enforces.
    pub label: &'static str,
    /// The intrinsic severity every finding for this code carries.
    pub severity: Severity,
    /// The default level a consumer reports this code at, absent an explicit
    /// `[lints]` override.
    pub default_level: LintLevel,
}

use LintLevel::{Allow, Deny, Warn};

/// Every code in the catalog, ordered by number. Append-only within each
/// thousand-block family. The fourth column is the rustc-style default
/// level: `Deny` (contract violation → error), `Warn` (suspicious but
/// executable → warning), `Allow` (opt-in perf/style opinion → off).
#[rustfmt::skip]
const ENTRIES: &[(u16, &str, Severity, LintLevel)] = &[
    (1001, "a table reference names a known table", Severity::Error, Deny),
    (1002, "a field reference names a declared field of its row's table (schemafull only; FLEXIBLE subtrees exempt)", Severity::Error, Deny),
    (1012, "a schema-object reference names a known object of that kind", Severity::Error, Deny),
    (1021, "REMOVE removes something that exists", Severity::Warning, Warn),
    (1022, "a definition does not silently redefine (OVERWRITE states intent)", Severity::Warning, Warn),
    (1023, "FETCH names something that can hold records", Severity::Error, Warn),
    (1024, "SPLIT names a collection field", Severity::Error, Warn),
    (1025, "a subfield is declared under an object-shaped parent", Severity::Error, Deny),
    (1027, "an index-backed operator has its supporting index", Severity::Error, Deny),
    (1029, "each index covers a distinct field set", Severity::Warning, Warn),
    (1032, "DEFINE ANALYZER components name known tokenizers/filters/languages", Severity::Error, Deny),
    (1033, "a DEFINE FIELD clause is one the field it targets accepts (`id` rejects VALUE/READONLY/COMPUTED/DEFAULT ALWAYS)", Severity::Error, Deny),
    (2001, "a value written to a field inhabits the field's declared type", Severity::Error, Deny),
    (2004, "the operands make sense together for the operator", Severity::Error, Deny),
    (2005, "a condition position expects a boolean", Severity::Warning, Warn),
    (2007, "a cast names a known type", Severity::Error, Deny),
    (2008, "a conversion can succeed", Severity::Error, Deny),
    (2012, "a body returns what it declares", Severity::Error, Deny),
    (2015, "a value-requiring position gets a value that is always present", Severity::Warning, Warn),
    (2017, "ORDER BY keys name fields available on the result rows (or RAND())", Severity::Error, Deny),
    (2018, "LIMIT/START take a non-negative integer", Severity::Error, Deny),
    (2019, "TIMEOUT takes a duration", Severity::Error, Deny),
    (2020, "KILL takes a live-query uuid", Severity::Error, Deny),
    (2021, "SHOW SINCE takes a versionstamp or datetime", Severity::Error, Deny),
    (2022, "FOR iterates something iterable", Severity::Error, Deny),
    (2025, "READONLY fields are written only at creation", Severity::Error, Deny),
    (2026, "computed (VALUE-clause) fields are not hand-assigned", Severity::Warning, Warn),
    (2030, "index/filter/splat apply to collections", Severity::Error, Deny),
    (2031, "a regex literal compiles", Severity::Error, Deny),
    (2032, "literal content is valid for its kind", Severity::Error, Deny),
    (2033, "PATCH operations are well-formed", Severity::Error, Deny),
    (2034, "required fields are provided at creation", Severity::Error, Deny),
    (2035, "DEFINE ANALYZER filter arguments are valid", Severity::Error, Deny),
    (2036, "GeoJSON literals have their declared shape", Severity::Error, Deny),
    (2037, "a field's DEFAULT satisfies its own ASSERT", Severity::Error, Deny),
    (3001, "a step traverses a relation table", Severity::Error, Deny),
    (3002, "the usage matches the relation's declared shape (`in`->edge->`out`)", Severity::Error, Deny),
    (3004, "a FROM-position chain is complete (edge->target pairs)", Severity::Error, Warn),
    (3009, "a traversal starts from records", Severity::Error, Deny),
    (3011, "graph recursion is bounded", Severity::Warning, Warn),
    (4001, "clause not valid on this statement", Severity::Error, Deny),
    (4002, "SELECT VALUE with multiple projections", Severity::Error, Deny),
    (4003, "ONLY on a table-wide target without LIMIT 1", Severity::Error, Deny),
    (4004, "INSERT tuple column/value count mismatch", Severity::Error, Deny),
    (4005, "BREAK/CONTINUE outside a loop", Severity::Error, Deny),
    (4006, "unreachable statements after RETURN/BREAK/THROW", Severity::Warning, Warn),
    (4007, "transaction pairing contract: BEGIN opens exactly one transaction that COMMIT/CANCEL closes", Severity::Error, Deny),
    (4009, "LIVE SELECT with unsupported clause", Severity::Error, Deny),
    (4010, "duplicate SET target in one statement", Severity::Warning, Warn),
    (4011, "duplicate projection key/alias", Severity::Warning, Warn),
    (4012, "OMIT without a wildcard projection", Severity::Warning, Warn),
    (4013, "GROUP BY field not in projections", Severity::Warning, Warn),
    (4016, "empty block", Severity::Hint, Allow),
    (4017, "block ends with LET — its value is NONE", Severity::Warning, Warn),
    (4018, "side-effecting subquery in read position", Severity::Warning, Warn),
    (4019, "CREATE/INSERT on a relation table without `in`/`out`", Severity::Warning, Warn),
    (4020, "RETURN mode meaningless for the statement", Severity::Warning, Warn),
    (4021, "SHOW CHANGES on a table without CHANGEFEED", Severity::Error, Warn),
    (4022, "SELECT from a DROP table", Severity::Warning, Warn),
    (
        4023,
        "count() without GROUP BY yields 1 per row, not a total",
        Severity::Warning,
        Warn,
    ),
    (4024, "an IF branch is unreachable — its guard provably folds to a constant", Severity::Warning, Warn),
    (4025, "a wildcard projection cannot be aggregated by a GROUP clause", Severity::Error, Deny),
    (4026, "a filtered ONLY has no provable single-row target", Severity::Warning, Warn),
    (5001, "a call resolves to a function that exists", Severity::Error, Deny),
    (5002, "a call matches the function's signature", Severity::Error, Deny),
    (5005, "a const argument satisfies the function's value contract", Severity::Error, Deny),
    (5009, "`fn::` definitions terminate (no direct/mutual recursion cycles)", Severity::Warning, Warn),
    (5010, "events do not trigger themselves (directly or in a cycle)", Severity::Warning, Warn),
    (6001, "conflicting constraints on one param", Severity::Error, Deny),
    (6002, "param shadows a DEFINE PARAM with a different kind", Severity::Warning, Warn),
    (6003, "unresolvable dynamic construct (analyzer limitation)", Severity::Hint, Allow),
    (6004, "param used before its LET in source order", Severity::Warning, Warn),
    (6005, "context param used outside its context", Severity::Error, Deny),
    (6006, "host-declared type contradicts query constraint", Severity::Error, Deny),
    (6007, "assignment to a protected parameter", Severity::Error, Deny),
    (7001, "unused LET binding", Severity::Warning, Allow),
    (7002, "LET shadowing", Severity::Hint, Allow),
    (7003, "mixed-kind array literal", Severity::Hint, Allow),
    (7004, "control flow is decided by a constant", Severity::Warning, Warn),
    (
        7005,
        "a comparison against a closed literal set must be able to match",
        Severity::Warning,
        Warn,
    ),
    (7006, "empty IN/CONTAINS list", Severity::Warning, Warn),
    (7007, "SELECT * with explicit fields", Severity::Hint, Warn),
    (7008, "schemaless table in a typed workspace", Severity::Hint, Allow),
    (7009, "whole-table UPDATE/DELETE without WHERE", Severity::Warning, Allow),
    (7011, "assignment to `id` in SET", Severity::Warning, Warn),
    (7012, "blocking or side-effecting call in a computed context", Severity::Warning, Warn),
    (
        7013,
        "a suppression directive names a catalog code (with a reason when required)",
        Severity::Warning,
        Warn,
    ),
    (7014, "whole-table SELECT without WHERE/LIMIT", Severity::Hint, Allow),
    (7015, "bare `SELECT *` (over-fetch / schema-drift brittleness)", Severity::Hint, Allow),
];

/// Every registered catalog entry, in code-number order. Consumers use
/// this to expand a family wildcard (e.g. `7xxx`) into its concrete codes.
pub fn all() -> impl Iterator<Item = CatalogEntry> {
    ENTRIES
        .iter()
        .map(|(number, label, severity, default_level)| CatalogEntry {
            number: *number,
            label,
            severity: *severity,
            default_level: *default_level,
        })
}

/// Looks up a catalog entry by code number.
pub fn entry(number: u16) -> Option<CatalogEntry> {
    let index = ENTRIES
        .binary_search_by_key(&number, |(n, _, _, _)| *n)
        .ok()?;
    let (number, label, severity, default_level) = ENTRIES[index];
    Some(CatalogEntry {
        number,
        label,
        severity,
        default_level,
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

        let registered: Vec<(u16, Severity)> =
            ENTRIES.iter().map(|(n, _, s, _)| (*n, *s)).collect();
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
