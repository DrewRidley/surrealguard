//! Spans over text that is not all ASCII.
//!
//! Spans are byte offsets, and the text around them may hold two-byte (`é`),
//! three-byte (CJK, `⟨⟩`) and four-byte (emoji) characters. Every span the
//! front end produces must still land on a UTF-8 character boundary — that
//! is what makes `&text[span]` safe — and must point at the text it claims
//! to. (The byte → UTF-16 line/column conversion the LSP protocol needs is
//! not this crate's: `crates/syntax/src/source.rs` carries only `SourceId`,
//! and the conversion lives in `crates/lsp/src/text.rs` with its own tests.)

mod support;

use surrealguard_syntax::ast::{Expr, IdiomPart, Literal, Spanned, Statement};
use surrealguard_syntax::highlight;
use surrealguard_syntax::lower::lower_statements;
use surrealguard_syntax::parse::parse_source;
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::ByteRange;

use support::ast_walk::{check_span, collect};

/// Parses and lowers `text`, checking every span (AST, diagnostics,
/// highlight tokens) is in bounds and on a boundary. Returns the statements.
fn lowered(text: &str) -> Vec<Spanned<Statement>> {
    let parsed = parse_source(SourceId::new("mb"), text).expect("parses");
    let statements = lower_statements(&parsed);
    let collected = collect(&statements);
    for (what, span) in &collected.spans {
        check_span(text, what, *span)
            .unwrap_or_else(|violation| panic!("{violation}\ninput: {text:?}"));
    }
    for diagnostic in parsed.syntax_diagnostics() {
        check_span(text, "diagnostic", diagnostic.span().range())
            .unwrap_or_else(|violation| panic!("{violation}\ninput: {text:?}"));
    }
    for token in highlight::tokens(&parsed) {
        let range =
            ByteRange::new(token.range.start as u32, token.range.end as u32).expect("ordered");
        check_span(text, "token", range)
            .unwrap_or_else(|violation| panic!("{violation}\ninput: {text:?}"));
    }
    statements
}

fn slice(text: &str, span: ByteRange) -> &str {
    &text[span.start() as usize..span.end() as usize]
}

fn select(statements: &[Spanned<Statement>]) -> &surrealguard_syntax::ast::SelectStmt {
    match &statements[0].node {
        Statement::Select(select) => select,
        other => panic!("expected SELECT, got {other:?}"),
    }
}

#[test]
fn spans_after_two_byte_characters_point_at_the_right_text() {
    // `é` is two bytes; everything after it is shifted by one relative to
    // the character count. (A bare identifier is ASCII-only in SurrealQL —
    // non-ASCII names are backtick- or `⟨⟩`-quoted.)
    let text = "SELECT name FROM `café` WHERE city = 'Zürich' LIMIT 5;";
    let statements = lowered(text);
    let select = select(&statements);

    assert_eq!(slice(text, select.from[0].span), "`café`");
    let where_clause = select.where_clause.as_ref().expect("where");
    assert_eq!(slice(text, where_clause.span), "city = 'Zürich'");
    let Expr::Binary { rhs, .. } = &where_clause.node else {
        panic!("expected binary, got {:?}", where_clause.node);
    };
    assert_eq!(slice(text, rhs.span), "'Zürich'");
    assert_eq!(rhs.node, Expr::Literal(Literal::String("Zürich".into())));
    assert_eq!(slice(text, select.limit.as_ref().expect("limit").span), "5");
}

#[test]
fn spans_after_emoji_point_at_the_right_text() {
    // Two four-byte characters and a ZWJ sequence before the clause that
    // matters.
    let text = "LET $greeting = '👋 hello 👨‍👩‍👧';\nSELECT * FROM person WHERE name = $greeting;";
    let statements = lowered(text);
    assert_eq!(statements.len(), 2);

    let Statement::Let(let_stmt) = &statements[0].node else {
        panic!("expected LET, got {:?}", statements[0].node);
    };
    assert_eq!(slice(text, let_stmt.name.span), "$greeting");
    assert_eq!(slice(text, let_stmt.value.span), "'👋 hello 👨‍👩‍👧'");

    let second = text.find("SELECT").expect("second statement");
    assert_eq!(statements[1].span.start() as usize, second);
    let select = match &statements[1].node {
        Statement::Select(select) => select,
        other => panic!("expected SELECT, got {other:?}"),
    };
    let where_clause = select.where_clause.as_ref().expect("where");
    assert_eq!(slice(text, where_clause.span), "name = $greeting");
    let Expr::Binary { lhs, op, rhs } = &where_clause.node else {
        panic!("expected binary, got {:?}", where_clause.node);
    };
    assert_eq!(slice(text, lhs.span), "name");
    assert_eq!(slice(text, op.span), "=");
    assert_eq!(slice(text, rhs.span), "$greeting");
}

#[test]
fn cjk_identifiers_and_strings_keep_their_spans() {
    // CJK in a quoted identifier, a string object key, a string, a comment,
    // and an angle-quoted record id — every one three bytes per character.
    let text = "-- 日本語のコメント\nCREATE `商品`:⟨東京⟩ CONTENT { \"名前\": '寿司', price: 12 };";
    let statements = lowered(text);
    assert_eq!(statements.len(), 1, "{statements:#?}");
    let Statement::Create(create) = &statements[0].node else {
        panic!("expected CREATE, got {:?}", statements[0].node);
    };
    assert_eq!(
        statements[0].span.start() as usize,
        text.find("CREATE").expect("CREATE")
    );

    let Expr::RecordId { table, id, .. } = &create.targets[0].node else {
        panic!("expected record id, got {:?}", create.targets[0].node);
    };
    assert_eq!(slice(text, table.span), "`商品`");
    assert_eq!(slice(text, *id), "⟨東京⟩");

    let Some(surrealguard_syntax::ast::DataClause::Content(content)) = &create.data else {
        panic!("expected CONTENT, got {:?}", create.data);
    };
    let Expr::Object(fields) = &content.node else {
        panic!("expected object, got {:?}", content.node);
    };
    assert_eq!(fields[0].0.node, "名前");
    assert_eq!(slice(text, fields[0].0.span), "\"名前\"");
    assert_eq!(slice(text, fields[0].1.span), "'寿司'");
    assert_eq!(
        fields[0].1.node,
        Expr::Literal(Literal::String("寿司".into()))
    );
    assert_eq!(slice(text, fields[1].1.span), "12");
}

#[test]
fn idiom_parts_after_multibyte_text_are_individually_spanned() {
    let text = "SELECT ->likes->post.`título`, ⟨名前⟩.length() FROM person:⟨ünïcödé⟩;";
    let statements = lowered(text);
    let select = select(&statements);

    let surrealguard_syntax::ast::Projection::Expr { expr, .. } = &select.projections[0] else {
        panic!("expected expr projection, got {:?}", select.projections[0]);
    };
    let Expr::Idiom(idiom) = &expr.node else {
        panic!("expected idiom, got {:?}", expr.node);
    };
    let texts: Vec<&str> = idiom
        .parts
        .iter()
        .map(|part| slice(text, part.span))
        .collect();
    // A field part spans the name token itself, not the `.` before it.
    assert_eq!(texts, ["->likes", "->post", "`título`"]);
    assert!(matches!(&idiom.parts[2].node, IdiomPart::Field(name) if name == "`título`"));

    let surrealguard_syntax::ast::Projection::Expr { expr, .. } = &select.projections[1] else {
        panic!("expected expr projection, got {:?}", select.projections[1]);
    };
    let Expr::Idiom(idiom) = &expr.node else {
        panic!("expected idiom, got {:?}", expr.node);
    };
    assert!(matches!(&idiom.parts[0].node, IdiomPart::Field(name) if name == "⟨名前⟩"));
    assert!(
        matches!(&idiom.parts[1].node, IdiomPart::Method { name, .. } if name.node == "length")
    );

    let Expr::RecordId { table, id, .. } = &select.from[0].node else {
        panic!("expected record id, got {:?}", select.from[0].node);
    };
    assert_eq!(slice(text, table.span), "person");
    assert_eq!(slice(text, *id), "⟨ünïcödé⟩");
}

/// Syntax diagnostics over multi-byte text: the error is reported from the
/// byte the parser stopped at, and every edge is a character boundary.
#[test]
fn diagnostics_over_multibyte_text_land_on_boundaries() {
    let text = "SELECT * FROM ⟨東京⟩ WHERE ⟨名前⟩ = '寿司";
    let parsed = parse_source(SourceId::new("mb"), text).expect("parses");
    assert!(parsed.has_error());
    let diagnostics = parsed.syntax_diagnostics();
    assert!(!diagnostics.is_empty());
    for diagnostic in diagnostics {
        check_span(text, "diagnostic", diagnostic.span().range())
            .unwrap_or_else(|violation| panic!("{violation}\ninput: {text:?}"));
    }
    // The statement ends before the comparison; the outermost ERROR covers
    // the operator and the unterminated string, and a nested one covers
    // the CJK text the stray quote left behind. Both slice cleanly.
    let texts: Vec<&str> = diagnostics
        .iter()
        .map(|d| slice(text, d.span().range()))
        .collect();
    assert_eq!(texts, ["= '寿司", "寿司"]);
}

/// Every byte-prefix of a multi-byte source that *is* a char boundary is a
/// state the editor passes through; every span in each must be sane. (A
/// prefix cut inside a character never reaches the parser: the editor's
/// buffer is a `String`.)
#[test]
fn every_char_prefix_of_multibyte_source_lowers_with_sane_spans() {
    let text = "SELECT ->likes->post.`título` AS `標題`, ⟨名前⟩.length() FROM person:⟨ünïcödé⟩ WHERE tag = '😀' LIMIT 3;";
    for (end, _) in text.char_indices().chain([(text.len(), ' ')]) {
        lowered(&text[..end]);
    }
}

/// The highlighter's token ranges over the same text slice back to the
/// token text — the LSP converts them to UTF-16 columns by slicing.
#[test]
fn highlight_tokens_slice_to_their_text() {
    let text = "SELECT `名前` FROM ⟨café⟩ WHERE x = '😀' -- é\n;";
    let parsed = parse_source(SourceId::new("mb"), text).expect("parses");
    let tokens = highlight::tokens(&parsed);
    let texts: Vec<&str> = tokens.iter().map(|t| &text[t.range.clone()]).collect();
    assert!(texts.contains(&"`名前`"), "{texts:?}");
    assert!(texts.contains(&"⟨café⟩"), "{texts:?}");
    assert!(texts.contains(&"'😀'"), "{texts:?}");
    assert!(texts.contains(&"-- é"), "{texts:?}");
}
