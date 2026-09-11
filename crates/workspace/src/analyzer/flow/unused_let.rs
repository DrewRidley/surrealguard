//! 7001 — unused `LET` binding.
//!
//! A `LET $x = …` whose `$x` is never read in the remainder of its scope is
//! dead: the binding computes a value nothing consumes. This is an opt-in
//! style lint (catalog default `allow`), so it never fires on the default
//! oracle — it only surfaces when a workspace turns it on.
//!
//! Detection is a *textual* reference scan, chosen so the check can never
//! false-positive. A binding is "used" when the token `$name` appears
//! anywhere in the source spanned by the statements that follow the `LET` in
//! the same scope (which textually contains those statements' nested blocks
//! too). Over-counting — a `$name` inside a string literal, or a later
//! same-named binding — only ever makes the check treat a `LET` as *used*, so
//! the failure mode is a missed report, never a wrong one. References before
//! the `LET`, or in the binding's own value, are correctly excluded because
//! the scan starts after the `LET` statement.
//!
//! Scopes nest: each `{ … }` block, `FOR` body, and `IF`/`ELSE` branch is its
//! own statement sequence, walked recursively so a block-local `LET` is scoped
//! to that block. A binding referenced from a deeper nested block is found via
//! the textual scan of the enclosing later sibling.

use std::collections::HashMap;

use surrealql_analyzer_syntax::ast;
use surrealql_analyzer_syntax::source::SourceId;
use surrealql_analyzer_syntax::span::{ByteRange, SourceSpan};

use surrealql_analyzer_diagnostics::Finding;

pub(crate) fn check_unused_lets(
    statements: &[ast::Spanned<ast::Statement>],
    source: &SourceId,
    text: &str,
    diagnostics: &mut Vec<Finding>,
) {
    check_sequence(statements, source, text, diagnostics);
}

/// Checks one statement sequence (a scope) for unused `LET`s, then recurses
/// into every nested scope each statement introduces.
///
/// The scan is *per scope*, not per binding. Asking "does any later statement
/// mention `$x`?" by re-reading every later statement's text made a scope of N
/// statements cost O(N × text), so a document of unused `LET`s was quadratic in
/// its own size — 3,200 of them spent well over half of whole-document analysis
/// here. One pass over the scope's text instead records, for each `$name` token
/// in it, the last statement that mentions that name; every binding's question
/// is then a map lookup. Same answer, one pass.
fn check_sequence<'t>(
    statements: &[ast::Spanned<ast::Statement>],
    source: &SourceId,
    text: &'t str,
    diagnostics: &mut Vec<Finding>,
) {
    let mut last_mention: HashMap<&'t str, usize> = HashMap::new();
    for (index, statement) in statements.iter().enumerate() {
        for_each_param_token(slice(text, statement.span), |name| {
            last_mention.insert(name, index);
        });
    }

    for (index, statement) in statements.iter().enumerate() {
        if let ast::Statement::Let(let_stmt) = &statement.node {
            let name = let_stmt.name.node.as_str();
            let used = if is_plain_identifier(name) {
                last_mention.get(name).is_some_and(|last| *last > index)
            } else {
                // A name that is not a bare identifier is written quoted in the
                // source, so no `$name` token scan can recover it. Such a name
                // keeps the original rescan: exact, and never hot, because
                // parameters are named with identifiers.
                statements[index + 1..]
                    .iter()
                    .any(|following| references_param(slice(text, following.span), name))
            };
            if !used {
                let span = SourceSpan::new(source.clone(), let_stmt.name.span);
                diagnostics.push(
                    surrealql_analyzer_diagnostics::catalog::finding(
                        span,
                        7001,
                        format!("`${name}` is never used"),
                    )
                    .with_help("remove the binding, or read it later in this scope"),
                );
            }
        }
        for nested in nested_sequences(&statement.node) {
            check_sequence(nested, source, text, diagnostics);
        }
    }
}

/// The statement sequences a statement opens as nested scopes. Only the
/// statement-position scopes are enumerated; scopes buried inside expressions
/// (subqueries, closures) are not descended into, which merely narrows what
/// the lint reports and never causes a false positive.
fn nested_sequences(statement: &ast::Statement) -> Vec<&[ast::Spanned<ast::Statement>]> {
    match statement {
        ast::Statement::Block(block) => vec![&block.statements],
        ast::Statement::For(for_stmt) => vec![&for_stmt.body.statements],
        ast::Statement::IfElse(if_else) => {
            let mut out: Vec<&[ast::Spanned<ast::Statement>]> = if_else
                .branches
                .iter()
                .map(|branch| branch.body.statements.as_slice())
                .collect();
            if let Some(else_branch) = &if_else.else_branch {
                out.push(&else_branch.statements);
            }
            out
        }
        ast::Statement::Expr(expr) => match &expr.node {
            ast::Expr::Block(block) => vec![&block.statements],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Whether the token `$name` occurs in `haystack` as a whole parameter
/// reference (the character after the name is not an identifier character, so
/// `$name` does not match inside `$name_2`).
fn references_param(haystack: &str, name: &str) -> bool {
    let needle_len = name.len() + 1; // leading `$`
    let bytes = haystack.as_bytes();
    let mut search_from = 0;
    while let Some(offset) = haystack[search_from..].find('$') {
        let dollar = search_from + offset;
        let after_name = dollar + needle_len;
        if haystack[dollar + 1..].starts_with(name) {
            let boundary_ok = bytes
                .get(after_name)
                .is_none_or(|byte| !is_identifier_byte(*byte));
            if boundary_ok {
                return true;
            }
        }
        search_from = dollar + 1;
    }
    false
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Calls `visit` with the name of every `$name` parameter token in `haystack`.
///
/// This is [`references_param`] turned inside out, and answers exactly the same
/// question for every identifier name at once. `references_param(h, name)` holds
/// for a bare-identifier `name` precisely when some `$` in `h` is followed by
/// the maximal run of identifier bytes `name`: the run's bytes are all
/// identifier bytes (so any shorter prefix fails the boundary test) and the byte
/// past it is not one (so nothing longer matches either). Visiting each run is
/// therefore visiting each name the rescan would have found — including a run
/// cut short by the end of `haystack`, which the boundary test also admits.
fn for_each_param_token<'t>(haystack: &'t str, mut visit: impl FnMut(&'t str)) {
    let bytes = haystack.as_bytes();
    for (dollar, _) in bytes.iter().enumerate().filter(|(_, b)| **b == b'$') {
        let start = dollar + 1;
        let mut end = start;
        while end < bytes.len() && is_identifier_byte(bytes[end]) {
            end += 1;
        }
        if end > start {
            // Identifier bytes are ASCII, so both ends are char boundaries.
            visit(&haystack[start..end]);
        }
    }
}

/// Whether `name` is a bare identifier — the form a `$name` token in the source
/// can spell, and so the form the token scan can answer for.
fn is_plain_identifier(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(is_identifier_byte)
}

fn slice(text: &str, range: ByteRange) -> &str {
    text.get(range.start() as usize..range.end() as usize)
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_param_respects_word_boundaries() {
        assert!(references_param("RETURN $x;", "x"));
        assert!(references_param("RETURN $x + 1;", "x"));
        assert!(!references_param("RETURN $x_2;", "x"));
        assert!(!references_param("RETURN $y;", "x"));
        assert!(!references_param("RETURN 5;", "x"));
        // A longer name is not matched by a prefix search.
        assert!(references_param("RETURN $count;", "count"));
        assert!(!references_param("RETURN $counter;", "count"));
    }

    /// The token scan replaced a per-binding rescan, so what it finds must be
    /// exactly what the rescan would have said — for every identifier name, on
    /// every shape the rescan has an opinion about.
    #[test]
    fn token_scan_agrees_with_the_rescan_it_replaced() {
        let haystacks = [
            "RETURN $x;",
            "RETURN $x_2 + $x;",
            "RETURN $counter;",
            "RETURN 5;",
            "RETURN $$x;",
            "RETURN a$x;",
            "RETURN '$x' + \"$y\";",
            // A name run cut short by the end of the slice still counts: the
            // boundary test admits end-of-input.
            "RETURN $ro",
            "$",
            "$ x",
            "LET $café = 1; RETURN $café;",
            "SELECT * FROM t WHERE a = $p AND b = $p2;",
        ];
        let names = [
            "x", "x_2", "y", "count", "counter", "ro", "row", "p", "p2", "caf", "café",
        ];
        for haystack in haystacks {
            let mut found: Vec<&str> = Vec::new();
            for_each_param_token(haystack, |name| found.push(name));
            for name in names {
                // Only bare identifiers take the token-scan path; everything
                // else is routed to the rescan, which is the answer by
                // definition.
                if !is_plain_identifier(name) {
                    continue;
                }
                let scanned = found.contains(&name);
                assert_eq!(
                    scanned,
                    references_param(haystack, name),
                    "`${name}` in {haystack:?}: token scan said {scanned}"
                );
            }
        }
    }

    #[test]
    fn a_non_identifier_name_is_not_claimed_by_the_token_scan() {
        // A param name can hold bytes the token scan's run cannot reproduce:
        // its boundary test is ASCII-only, so a run stops at the first byte of
        // a multi-byte character and `café` would be recorded as `caf`. Such a
        // name must not be answered from the scan — it takes the rescan.
        assert!(!is_plain_identifier("my param"));
        assert!(!is_plain_identifier("café"));
        assert!(!is_plain_identifier(""));
        assert!(is_plain_identifier("row0"));
        // And the rescan is the one that gets it right.
        assert!(references_param("RETURN $café;", "café"));
    }
}
