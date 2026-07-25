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

use surrealguard_syntax::ast;
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use surrealguard_diagnostics::Finding;

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
fn check_sequence(
    statements: &[ast::Spanned<ast::Statement>],
    source: &SourceId,
    text: &str,
    diagnostics: &mut Vec<Finding>,
) {
    for (index, statement) in statements.iter().enumerate() {
        if let ast::Statement::Let(let_stmt) = &statement.node {
            let name = let_stmt.name.node.as_str();
            let used = statements[index + 1..]
                .iter()
                .any(|following| references_param(slice(text, following.span), name));
            if !used {
                let span = SourceSpan::new(source.clone(), let_stmt.name.span);
                diagnostics.push(
                    surrealguard_diagnostics::catalog::finding(
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
}
