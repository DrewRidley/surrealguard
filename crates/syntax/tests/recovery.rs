//! Error-recovery shapes the editor produces while a statement is typed.
//!
//! Each test pins one half-typed input: where the parse diagnostics point
//! (the `S0001` syntax-error family the diagnostics crate emits from these),
//! which statements lower to `Partial`, and that the *neighbouring* valid
//! statements still lower fully with the spans they would have had alone.
//! The property test in `robustness.rs` proves nothing panics; these prove
//! the recovery is the recovery we want.

mod support;

use surrealql_analyzer_syntax::ast::{Expr, Statement, TypeExpr};
use surrealql_analyzer_syntax::lower::lower_statements;
use surrealql_analyzer_syntax::parse::{
    parse_source, ParsedSource, SyntaxDiagnostic, SyntaxDiagnosticKind,
};
use surrealql_analyzer_syntax::source::SourceId;

use support::ast_walk::{check_span, collect, is_recovery_partial};

/// Parses, lowers, and checks the invariants every input must satisfy
/// (every span in bounds and on a char boundary; recovery `Partial`s only
/// when the tree has an error). Returns both halves for the test's own
/// assertions.
fn front_end(
    text: &str,
) -> (
    ParsedSource,
    Vec<surrealql_analyzer_syntax::ast::Spanned<Statement>>,
) {
    let parsed = parse_source(SourceId::new("recovery"), text).expect("tree-sitter returns a tree");
    let statements = lower_statements(&parsed);
    for diagnostic in parsed.syntax_diagnostics() {
        check_span(text, "syntax diagnostic", diagnostic.span().range())
            .unwrap_or_else(|violation| panic!("{violation}\ninput: {text:?}"));
    }
    let collected = collect(&statements);
    for (what, span) in &collected.spans {
        check_span(text, what, *span)
            .unwrap_or_else(|violation| panic!("{violation}\ninput: {text:?}"));
    }
    if !parsed.has_error() {
        assert!(
            !collected.partials.iter().any(is_recovery_partial),
            "clean tree but recovery Partial in {statements:#?}"
        );
    }
    (parsed, statements)
}

/// The byte range of `needle` in `text` (first occurrence), as `(start, end)`.
fn at(text: &str, needle: &str) -> (u32, u32) {
    let start = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {text:?}"));
    (start as u32, (start + needle.len()) as u32)
}

fn span_of<T>(spanned: &surrealql_analyzer_syntax::ast::Spanned<T>) -> (u32, u32) {
    (spanned.span.start(), spanned.span.end())
}

fn diagnostic_span(diagnostic: &SyntaxDiagnostic) -> (u32, u32) {
    let range = diagnostic.span().range();
    (range.start(), range.end())
}

fn missing_diagnostics(parsed: &ParsedSource) -> Vec<&SyntaxDiagnostic> {
    parsed
        .syntax_diagnostics()
        .iter()
        .filter(|d| d.kind() == SyntaxDiagnosticKind::MissingNode)
        .collect()
}

fn error_diagnostics(parsed: &ParsedSource) -> Vec<&SyntaxDiagnostic> {
    parsed
        .syntax_diagnostics()
        .iter()
        .filter(|d| d.kind() == SyntaxDiagnosticKind::ErrorNode)
        .collect()
}

fn is_partial(statement: &Statement) -> bool {
    matches!(statement, Statement::Partial(_))
}

#[test]
fn unterminated_string_collapses_only_its_statement() {
    let text = "SELECT * FROM person WHERE name = 'abc";
    let (parsed, statements) = front_end(text);

    assert!(parsed.has_error());
    // The stray quote is the error; the parser reads `abc` as a field.
    let errors = error_diagnostics(&parsed);
    assert_eq!(errors.len(), 1, "{:#?}", parsed.syntax_diagnostics());
    assert_eq!(diagnostic_span(errors[0]), at(text, "'"));

    assert_eq!(statements.len(), 1);
    assert!(is_partial(&statements[0].node), "{statements:#?}");
    assert_eq!(span_of(&statements[0]), (0, text.len() as u32));
}

#[test]
fn unterminated_block_reports_the_missing_brace_at_the_end() {
    let text = "LET $x = { RETURN 1;";
    let (parsed, statements) = front_end(text);

    let missing = missing_diagnostics(&parsed);
    assert_eq!(missing.len(), 1, "{:#?}", parsed.syntax_diagnostics());
    assert!(
        missing[0].message().contains("BraceClose"),
        "{}",
        missing[0].message()
    );
    // A MISSING node is zero-width, placed where the token should have been.
    let (start, end) = diagnostic_span(missing[0]);
    assert_eq!(start, end);
    assert_eq!(end, text.len() as u32);

    // The LET is broken; the block it holds is a recoverable container, so
    // the salvaged block still yields its well-formed RETURN.
    assert!(is_partial(&statements[0].node), "{statements:#?}");
    assert_eq!(span_of(&statements[0]), (0, text.len() as u32));
    let Statement::Block(block) = &statements[1].node else {
        panic!("expected the salvaged block, got {:?}", statements[1].node);
    };
    assert_eq!(block.statements.len(), 1);
    assert!(matches!(block.statements[0].node, Statement::Return(_)));
    assert_eq!(span_of(&block.statements[0]), at(text, "RETURN 1"));
}

#[test]
fn unterminated_parenthesis_and_bracket_are_partial() {
    for (text, closer) in [("RETURN (1 + 2", ")"), ("RETURN [1, 2", "]")] {
        let (parsed, statements) = front_end(text);
        assert!(parsed.has_error(), "{text:?} should not parse cleanly");
        let missing = missing_diagnostics(&parsed);
        assert!(
            !missing.is_empty(),
            "{text:?}: expected a MISSING {closer}, got {:#?}",
            parsed.syntax_diagnostics()
        );
        for diagnostic in &missing {
            let (start, end) = diagnostic_span(diagnostic);
            assert_eq!(start, end, "{text:?}: MISSING nodes are zero-width");
            assert_eq!(
                end,
                text.len() as u32,
                "{text:?}: the closer belongs at the end"
            );
        }
        assert_eq!(statements.len(), 1, "{text:?}: {statements:#?}");
        assert!(is_partial(&statements[0].node), "{text:?}: {statements:#?}");
    }
}

#[test]
fn a_broken_statement_leaves_both_neighbours_intact() {
    let text = "SELECT * FROM person; CREATE person SET name = ; RETURN 1;";
    let (parsed, statements) = front_end(text);

    let missing = missing_diagnostics(&parsed);
    assert_eq!(missing.len(), 1, "{:#?}", parsed.syntax_diagnostics());
    let (start, end) = diagnostic_span(missing[0]);
    assert_eq!(start, end);
    let (_, after_eq) = at(text, "name =");
    let (before_semi, _) = at(text, " ; RETURN");
    assert!(
        (after_eq..=before_semi + 1).contains(&start),
        "MISSING value should sit between `=` and `;`, at {start}"
    );

    assert_eq!(statements.len(), 3, "{statements:#?}");
    let Statement::Select(select) = &statements[0].node else {
        panic!("expected SELECT, got {:?}", statements[0].node);
    };
    assert_eq!(span_of(&statements[0]), at(text, "SELECT * FROM person"));
    assert!(matches!(&select.from[0].node, Expr::Table(name) if name.node == "person"));

    assert!(is_partial(&statements[1].node), "{:?}", statements[1].node);
    assert_eq!(
        span_of(&statements[1]),
        at(text, "CREATE person SET name =")
    );

    let Statement::Return(ret) = &statements[2].node else {
        panic!("expected RETURN, got {:?}", statements[2].node);
    };
    assert_eq!(span_of(&statements[2]), at(text, "RETURN 1"));
    let (one, _) = at(text, "1;");
    assert_eq!(span_of(ret.value.as_ref().expect("value")), (one, one + 1));
}

/// tree-sitter often nests the statement *after* an error inside the broken
/// statement's subtree. The lowering salvages it: the broken statement's
/// `Partial` still spans the absorbed text, and the salvaged neighbour
/// follows with its own exact span.
#[test]
fn a_statement_absorbed_by_a_broken_neighbour_is_salvaged() {
    let text = "SELECT * FROM person; SELECT * FROM ; RETURN 1;";
    let (parsed, statements) = front_end(text);
    assert!(parsed.has_error());

    assert_eq!(statements.len(), 3, "{statements:#?}");
    assert!(matches!(statements[0].node, Statement::Select(_)));
    assert_eq!(span_of(&statements[0]), at(text, "SELECT * FROM person"));
    assert!(is_partial(&statements[1].node));
    assert_eq!(
        span_of(&statements[1]),
        at(text, "SELECT * FROM ; RETURN 1")
    );
    assert!(matches!(statements[2].node, Statement::Return(_)));
    assert_eq!(span_of(&statements[2]), at(text, "RETURN 1"));
}

#[test]
fn select_from_nothing_is_a_missing_source() {
    let text = "SELECT * FROM ;";
    let (parsed, statements) = front_end(text);

    let missing = missing_diagnostics(&parsed);
    assert_eq!(missing.len(), 1, "{:#?}", parsed.syntax_diagnostics());
    assert!(
        missing[0].message().contains("Ident"),
        "{}",
        missing[0].message()
    );
    let (start, end) = diagnostic_span(missing[0]);
    assert_eq!(start, end);
    let (_, after_from) = at(text, "FROM");
    assert!((after_from..=after_from + 1).contains(&start), "at {start}");

    assert_eq!(statements.len(), 1);
    assert!(is_partial(&statements[0].node));
    assert_eq!(span_of(&statements[0]), at(text, "SELECT * FROM"));
}

#[test]
fn define_field_with_everything_missing_is_one_error() {
    let text = "DEFINE FIELD ON";
    let (parsed, statements) = front_end(text);

    // Nothing here can be a DefineStatement yet, so the parser wraps the
    // whole thing in one ERROR; the diagnostic covers all of it.
    let errors = error_diagnostics(&parsed);
    assert_eq!(errors.len(), 1, "{:#?}", parsed.syntax_diagnostics());
    assert_eq!(diagnostic_span(errors[0]), (0, text.len() as u32));

    assert_eq!(statements.len(), 1, "{statements:#?}");
    let Statement::Partial(node) = &statements[0].node else {
        panic!("expected Partial, got {:?}", statements[0].node);
    };
    assert_eq!(node.cst_kind, "ERROR");
    assert_eq!(span_of(&statements[0]), (0, text.len() as u32));
}

#[test]
fn define_table_without_a_name_is_a_missing_ident() {
    let text = "DEFINE TABLE";
    let (parsed, statements) = front_end(text);

    let missing = missing_diagnostics(&parsed);
    assert_eq!(missing.len(), 1, "{:#?}", parsed.syntax_diagnostics());
    assert!(missing[0].message().contains("Ident"));
    let (start, end) = diagnostic_span(missing[0]);
    assert_eq!(start, end);
    assert_eq!(end, text.len() as u32);

    assert_eq!(statements.len(), 1);
    let Statement::Partial(node) = &statements[0].node else {
        panic!("expected Partial, got {:?}", statements[0].node);
    };
    // The statement collapses as a whole: the Partial names the statement,
    // the diagnostic names the missing piece.
    assert_eq!(node.cst_kind, "DefineStatement");
}

#[test]
fn arrow_with_nothing_after_it() {
    // Mid-expression: the statement before the arrow is complete, so it
    // lowers; the dangling arrow is its own ERROR.
    let text = "RETURN person:one->;";
    let (parsed, statements) = front_end(text);
    let errors = error_diagnostics(&parsed);
    assert_eq!(errors.len(), 1, "{:#?}", parsed.syntax_diagnostics());
    assert_eq!(diagnostic_span(errors[0]), at(text, "->"));
    assert_eq!(statements.len(), 2, "{statements:#?}");
    assert!(matches!(statements[0].node, Statement::Return(_)));
    assert_eq!(span_of(&statements[0]), at(text, "RETURN person:one"));
    let Statement::Partial(node) = &statements[1].node else {
        panic!("expected Partial, got {:?}", statements[1].node);
    };
    assert_eq!(node.cst_kind, "ERROR");
    assert_eq!(span_of(&statements[1]), at(text, "->"));

    // In projection position, nothing can be a statement: the whole input
    // is one ERROR and lowers to one Partial covering it.
    let text = "SELECT ->";
    let (parsed, statements) = front_end(text);
    let errors = error_diagnostics(&parsed);
    assert_eq!(errors.len(), 1, "{:#?}", parsed.syntax_diagnostics());
    assert_eq!(diagnostic_span(errors[0]), (0, text.len() as u32));
    assert_eq!(statements.len(), 1, "{statements:#?}");
    assert!(is_partial(&statements[0].node));
    assert_eq!(span_of(&statements[0]), (0, text.len() as u32));

    // With more text after it the parser gives up on the root itself: the
    // tree's root is the ERROR. Lowering must still return only Partials,
    // each inside the text.
    let text = "SELECT ->  FROM person;";
    let (parsed, statements) = front_end(text);
    assert_eq!(parsed.root_kind(), "ERROR");
    assert!(!parsed.syntax_diagnostics().is_empty());
    assert!(!statements.is_empty());
    assert!(
        statements.iter().all(|s| is_partial(&s.node)),
        "{statements:#?}"
    );
}

#[test]
fn half_typed_where_comparison_is_a_missing_operand() {
    let text = "SELECT * FROM person WHERE a =";
    let (parsed, statements) = front_end(text);

    let missing = missing_diagnostics(&parsed);
    assert_eq!(missing.len(), 1, "{:#?}", parsed.syntax_diagnostics());
    let (start, end) = diagnostic_span(missing[0]);
    assert_eq!(start, end);
    assert_eq!(end, text.len() as u32);

    assert_eq!(statements.len(), 1);
    assert!(is_partial(&statements[0].node));
    assert_eq!(span_of(&statements[0]), (0, text.len() as u32));
}

/// A bare `$` is not a parameter, and the parser stops the statement before
/// the operator that leads to it. The SELECT that remains (`WHERE id`) is
/// well-formed SurrealQL, so it lowers — with a span that ends *before* the
/// error, which is what tells the analyzer the WHERE it sees is not the one
/// the user is typing.
#[test]
fn dangling_param_sigil_is_an_error_after_the_statement() {
    let text = "SELECT * FROM person WHERE id = $";
    let (parsed, statements) = front_end(text);

    let errors = error_diagnostics(&parsed);
    assert_eq!(errors.len(), 1, "{:#?}", parsed.syntax_diagnostics());
    assert_eq!(diagnostic_span(errors[0]), at(text, "= $"));

    assert_eq!(statements.len(), 2, "{statements:#?}");
    let Statement::Select(select) = &statements[0].node else {
        panic!("expected SELECT, got {:?}", statements[0].node);
    };
    assert_eq!(
        span_of(&statements[0]),
        at(text, "SELECT * FROM person WHERE id")
    );
    assert_eq!(
        span_of(select.where_clause.as_ref().expect("where")),
        at(text, "id")
    );
    let Statement::Partial(node) = &statements[1].node else {
        panic!("expected Partial, got {:?}", statements[1].node);
    };
    assert_eq!(node.cst_kind, "ERROR");
    assert_eq!(span_of(&statements[1]), at(text, "= $"));
}

/// `<in>` is not a type, but the grammar accepts any identifier in cast
/// position: this is *not* a syntax error. It lowers as a cast to the type
/// named `in`, and rejecting that name is the analyzer's job.
#[test]
fn invalid_cast_target_is_a_named_type_not_a_syntax_error() {
    let text = "RETURN <in> 1;";
    let (parsed, statements) = front_end(text);

    assert!(!parsed.has_error());
    assert!(parsed.syntax_diagnostics().is_empty());
    assert_eq!(statements.len(), 1);
    let Statement::Return(ret) = &statements[0].node else {
        panic!("expected RETURN, got {:?}", statements[0].node);
    };
    let value = ret.value.as_ref().expect("value");
    let Expr::Cast { ty, expr } = &value.node else {
        panic!("expected a cast, got {:?}", value.node);
    };
    let TypeExpr::Name(name) = &ty.node else {
        panic!("expected a type name, got {:?}", ty.node);
    };
    assert_eq!(name.node, "in");
    assert_eq!(span_of(name), at(text, "in"));
    assert_eq!(span_of(expr), at(text, "1"));
}

#[test]
fn unclosed_angle_quoted_record_id_is_an_error_after_the_table() {
    let text = "SELECT * FROM person:⟨abc";
    let (parsed, statements) = front_end(text);

    // `⟨` is three bytes; every diagnostic must still land on a boundary,
    // and the outermost one starts at the colon.
    let errors = error_diagnostics(&parsed);
    assert!(!errors.is_empty(), "{:#?}", parsed.syntax_diagnostics());
    let (colon, _) = at(text, ":");
    assert_eq!(diagnostic_span(errors[0]), (colon, text.len() as u32));
    for diagnostic in &errors {
        let (start, end) = diagnostic_span(diagnostic);
        assert!(text.is_char_boundary(start as usize) && text.is_char_boundary(end as usize));
    }

    assert_eq!(statements.len(), 2, "{statements:#?}");
    let Statement::Select(select) = &statements[0].node else {
        panic!("expected SELECT, got {:?}", statements[0].node);
    };
    assert!(matches!(&select.from[0].node, Expr::Table(name) if name.node == "person"));
    assert_eq!(span_of(&statements[0]), at(text, "SELECT * FROM person"));
    assert!(is_partial(&statements[1].node));
    assert_eq!(span_of(&statements[1]), (colon, text.len() as u32));
}
