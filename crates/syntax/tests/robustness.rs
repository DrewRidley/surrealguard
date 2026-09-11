//! Property test: parse + lower never panic on half-typed input, and every
//! span they produce points into the text.
//!
//! The LSP runs `parse_source` and `lower_statements` on every keystroke, so
//! the input they see is mostly *not* SurrealQL: a statement with the cursor
//! in the middle, a truncated file, a paste of something else entirely. The
//! generator here produces that shape — random bytes, valid corpus
//! statements with single-character edits at random offsets, truncations,
//! and concatenations of all of the above — and checks three invariants on
//! each:
//!
//! 1. `parse_source` and `lower_statements` return (no panic).
//! 2. Every span in the lowered AST has `start <= end <= text.len()` and both
//!    ends on a UTF-8 character boundary — so slicing the text by any span is
//!    safe, and so is converting it to an LSP position.
//! 3. A `Partial` whose `cst_kind` is `ERROR` or `MISSING …` appears only
//!    when the CST really has an `ERROR`/`MISSING` node. (Other `Partial`s
//!    are the lowering's explicit "not modeled" marker — `DefineStmt::Other`
//!    for `DEFINE ACCESS`, `RemoveTarget::Other`, `Statement::Partial` for a
//!    statement kind with no lowering, `TypeExpr::Partial` for unmodeled type
//!    syntax, `GraphStep::unmodeled` — and can legitimately appear in a clean
//!    parse.)
//!
//! Runs 256 cases by default; `PROPTEST_CASES=<n>` raises it.

mod support;

use std::path::Path;
use std::sync::LazyLock;

use proptest::prelude::*;
use proptest::test_runner::Config;
use surrealql_analyzer_syntax::highlight;
use surrealql_analyzer_syntax::lower::lower_statements;
use surrealql_analyzer_syntax::parse::{parse_source, ParsedSource};
use surrealql_analyzer_syntax::source::SourceId;

use support::ast_walk::{check_span, collect, is_recovery_partial};
use support::{corpus_path, load_corpus};

/// Every known-valid piece of SurrealQL the generator can start from: the
/// conformance corpus entries plus each statement of the workspace corpus.
static SEEDS: LazyLock<Vec<String>> = LazyLock::new(|| {
    let mut seeds = load_corpus(&corpus_path());
    let corpus_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../workspace/tests/corpus");
    let mut files = Vec::new();
    collect_surql_files(&corpus_dir, &mut files);
    assert!(
        !files.is_empty(),
        "no .surql files under {} — the generator would lose its realistic seeds",
        corpus_dir.display()
    );
    for file in files {
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("read {}: {error}", file.display()));
        // Whole files are realistic too, but the statement granularity is
        // what the editor mostly has half-typed.
        seeds.extend(
            text.split_inclusive(';')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
        );
        seeds.push(text);
    }
    seeds
});

fn collect_surql_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_surql_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "surql") {
            out.push(path);
        }
    }
}

/// One edit of the "half-typed" kind, applied at a character offset chosen
/// modulo the text's length so any `usize` is a valid position.
#[derive(Clone, Debug)]
enum Edit {
    Delete(usize),
    Insert(usize, char),
    Truncate(usize),
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        any::<usize>().prop_map(Edit::Delete),
        (any::<usize>(), typed_char()).prop_map(|(at, ch)| Edit::Insert(at, ch)),
        any::<usize>().prop_map(Edit::Truncate),
    ]
}

/// The characters a keystroke most plausibly inserts: SurrealQL punctuation
/// (the ones that open and close things weigh most), ASCII, and some
/// multi-byte text so byte/char boundaries get exercised.
fn typed_char() -> impl Strategy<Value = char> {
    prop_oneof![
        4 => prop::sample::select(
            "(){}[]<>⟨⟩'\"`$:;,.-=+*/|?!@#&^~\\ \n\t".chars().collect::<Vec<_>>()
        ),
        2 => any::<char>().prop_filter("ascii", char::is_ascii),
        1 => prop::sample::select("é漢字😀ñ🚀中文\u{200d}\u{fe0f}".chars().collect::<Vec<_>>()),
    ]
}

fn apply(text: &str, edit: &Edit) -> String {
    let boundaries: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();
    let pick = |at: usize| boundaries[at % boundaries.len()];
    match edit {
        Edit::Delete(at) => {
            let start = pick(*at);
            let mut out = String::from(&text[..start]);
            out.extend(text[start..].chars().skip(1));
            out
        }
        Edit::Insert(at, ch) => {
            let start = pick(*at);
            let mut out = String::from(&text[..start]);
            out.push(*ch);
            out.push_str(&text[start..]);
            out
        }
        Edit::Truncate(at) => text[..pick(*at)].to_owned(),
    }
}

/// A valid seed with zero to four edits applied.
fn half_typed() -> impl Strategy<Value = String> {
    (
        prop::sample::select(&SEEDS[..]),
        prop::collection::vec(edit(), 0..=4),
    )
        .prop_map(|(seed, edits)| edits.iter().fold(seed.clone(), |text, e| apply(&text, e)))
}

/// Text with no relation to SurrealQL: random bytes made into a string
/// (lossily, so invalid sequences become U+FFFD) and random Unicode.
fn noise() -> impl Strategy<Value = String> {
    prop_oneof![
        prop::collection::vec(any::<u8>(), 0..64)
            .prop_map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
        "\\PC{0,48}",
        prop::collection::vec(typed_char(), 0..48).prop_map(|chars| chars.into_iter().collect()),
    ]
}

fn piece() -> impl Strategy<Value = String> {
    prop_oneof![
        6 => half_typed(),
        2 => noise(),
        1 => prop::sample::select(&SEEDS[..]).prop_map(|s| s.clone()),
    ]
}

/// The generator: one to three pieces joined by a statement-ish separator.
fn source_text() -> impl Strategy<Value = String> {
    (
        prop::collection::vec(piece(), 1..=3),
        prop::sample::select(vec!["", " ", ";", "\n", ";\n", " ; "]),
    )
        .prop_map(|(pieces, sep)| pieces.join(sep))
}

fn cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(256)
}

/// Runs the whole front end over `text` and checks every invariant.
/// Returns the first violation so the property reports it.
fn check(text: &str) -> Result<(), String> {
    let parsed: ParsedSource =
        parse_source(SourceId::new("prop"), text).map_err(|error| error.to_string())?;
    let tree_has_error = parsed.has_error();

    for diagnostic in parsed.syntax_diagnostics() {
        let range = diagnostic.span().range();
        check_span(text, "syntax diagnostic", range)?;
    }
    if tree_has_error != !parsed.syntax_diagnostics().is_empty() {
        return Err(format!(
            "has_error() is {tree_has_error} but {} syntax diagnostics were collected",
            parsed.syntax_diagnostics().len()
        ));
    }

    let statements = lower_statements(&parsed);
    let collected = collect(&statements);
    for (what, span) in &collected.spans {
        check_span(text, what, *span)?;
    }
    if !tree_has_error {
        if let Some(node) = collected.partials.iter().find(|n| is_recovery_partial(n)) {
            return Err(format!(
                "clean parse (no ERROR/MISSING nodes) but lowering produced a recovery \
                 Partial {node:?}"
            ));
        }
    }

    // Statement spans come out in source order. (They may nest: a statement
    // tree-sitter absorbed into a broken neighbour's subtree is salvaged
    // *after* that neighbour's `Partial`, whose span still covers it.)
    for pair in statements.windows(2) {
        if pair[0].span.start() > pair[1].span.start() {
            return Err(format!(
                "statement spans out of source order: {:?} then {:?}",
                pair[0].span, pair[1].span
            ));
        }
    }

    // The highlighter runs on the same keystroke; its ranges obey the same
    // contract and must be disjoint and ascending.
    let tokens = highlight::tokens(&parsed);
    for token in &tokens {
        let range = surrealql_analyzer_syntax::span::ByteRange::new(
            token.range.start as u32,
            token.range.end as u32,
        )
        .map_err(|_| format!("reversed token range {:?}", token.range))?;
        check_span(text, "highlight token", range)?;
    }
    for pair in tokens.windows(2) {
        if pair[0].range.end > pair[1].range.start {
            return Err(format!(
                "highlight tokens overlap: {:?} then {:?}",
                pair[0].range, pair[1].range
            ));
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(Config {
        cases: cases(),
        // Failing inputs are written next to the crate so the bug is
        // reproducible; the input itself is in the assertion message too.
        ..Config::default()
    })]

    #[test]
    fn parse_and_lower_survive_half_typed_input(text in source_text()) {
        if let Err(violation) = check(&text) {
            prop_assert!(false, "{violation}\ninput: {text:?}");
        }
    }
}

/// The generator's own moving parts, so a silent generator bug cannot make
/// the property vacuous.
#[test]
fn edits_stay_on_char_boundaries() {
    let text = "SELECT é FROM 漢字 WHERE x = '😀';";
    let count = text.chars().count();
    for at in 0..=count + 3 {
        let deleted = apply(text, &Edit::Delete(at));
        assert_eq!(
            deleted.chars().count(),
            count - usize::from(at % (count + 1) < count)
        );
        let inserted = apply(text, &Edit::Insert(at, '⟨'));
        assert_eq!(inserted.chars().count(), count + 1);
        let truncated = apply(text, &Edit::Truncate(at));
        assert!(text.starts_with(&truncated));
    }
}

#[test]
fn the_generator_has_realistic_seeds() {
    // Conformance entries plus the workspace corpus statements.
    assert!(SEEDS.len() > 1_000, "only {} seeds", SEEDS.len());
    assert!(SEEDS.iter().any(|s| s.starts_with("DEFINE TABLE")));
    assert!(SEEDS.iter().any(|s| s.contains("->")));
}

/// A directed sweep no random generator reaches reliably: every prefix of a
/// handful of realistic statements. Each prefix is a state the editor is in
/// while the statement is typed.
#[test]
fn every_prefix_of_a_statement_parses_and_lowers() {
    let statements = [
        "SELECT name, ->likes->post.title AS titles FROM person WHERE age > 18 ORDER BY name LIMIT 10;",
        "DEFINE FIELD tags ON TABLE post TYPE array<string> DEFAULT [] PERMISSIONS FOR select WHERE published = true;",
        "LET $u = (SELECT * FROM ONLY person:⟨a b⟩ FETCH friends);",
        "CREATE person:one CONTENT { name: 'Ω', tags: [\"x\", 'y'], age: <int> $n };",
        "IF $x > 1 { RETURN d'2024-01-01T00:00:00Z'; } ELSE { THROW 'no 😀'; };",
        "DEFINE FUNCTION fn::greet($name: string) -> string { RETURN \"Hi \" + $name; };",
        "RELATE person:a->knows->person:b SET since = time::now() RETURN AFTER;",
        "INSERT INTO t (a, b) VALUES (1, 2), (3, 4) ON DUPLICATE KEY UPDATE a += 1;",
    ];
    for statement in statements {
        for (end, _) in statement.char_indices().chain([(statement.len(), ' ')]) {
            let prefix = &statement[..end];
            if let Err(violation) = check(prefix) {
                panic!("{violation}\ninput: {prefix:?}");
            }
        }
    }
}
