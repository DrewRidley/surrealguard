//! Classifying what the cursor sits in, robustly enough for text that does
//! not parse.
//!
//! # Why a token scan, not the CST
//!
//! Completion fires while the statement is *incomplete*, which is exactly the
//! input tree-sitter recovers worst on. `SELECT name FROM ▏` recovers well (an
//! empty `Ident` under the `SelectStatement`), but `SELECT ▏ FROM person`
//! collapses the whole statement into a single top-level `ERROR` node holding
//! one `Keyword` — every structural fact completion needs is gone. Anchoring
//! on the CST would therefore give excellent answers in some incomplete states
//! and *nothing* in others, unpredictably.
//!
//! So the backbone is a keyword-driven scan over [`super::lex`] tokens, which
//! degrades smoothly: an unfinished statement is still a head keyword, some
//! clause keywords, and some identifiers, and that is all the classifier
//! reads. The CST is consulted only where it adds information the token
//! stream cannot carry (see [`in_ignored_region`]) — never as the sole path to
//! an answer.
//!
//! # What the scan reconstructs
//!
//! 1. **Frames.** `(`/`[`/`{` push a frame; `;` restarts the statement inside
//!    the current frame. The innermost frame owns the cursor, so a subquery
//!    inside `array::len((SELECT …))` is classified on its own terms.
//! 2. **Statement bounds.** From the frame's statement start forward to the
//!    next depth-0 `;`, the frame's close, or end of input — *past the
//!    cursor*, which is what lets `SELECT ▏ FROM person` see its own `FROM`.
//! 3. **Head and clause.** The last statement-head keyword at depth 0 before
//!    the cursor (so `LET $x = SELECT … WHERE ▏` is classified as a SELECT),
//!    and the last clause keyword after it.
//! 4. **The preceding operator.** A comparison or assignment right before the
//!    cursor turns a field position into a value position, and supplies the
//!    expected kind from the left-hand idiom.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use surrealdb_types::Kind;
use surrealguard_syntax::parse::ParsedSource;

use super::lex::{tokenize, Token, TokenKind};
use crate::schema::SchemaIndex;

/// What the cursor sits in. The classification drives which candidate
/// families are offered and how they are weighted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextKind {
    /// A field name of the statement's row table(s): a projection entry,
    /// `OMIT`, `WHERE`, `SPLIT`/`GROUP`/`ORDER`/`FETCH`, a `SET`/`UNSET`
    /// target, or `DEFINE INDEX … FIELDS`.
    FieldName,
    /// A table name: after `FROM`/`INTO`, a `CREATE`/`UPDATE`/`UPSERT`/
    /// `DELETE`/`RELATE` target, or `DEFINE … ON`.
    TableName,
    /// The edge slot of a graph step (`$a->▏`, `->▏->company`). Only a
    /// `TYPE RELATION` table belongs here, so edges rank above plain tables.
    EdgeTable,
    /// A key inside a `CONTENT`/`MERGE` object literal.
    ObjectKey,
    /// A member after `.` on a receiver whose kind is known.
    Member,
    /// A member inside a `.{ … }` destructure.
    Destructure,
    /// A `$param` name.
    ParamName,
    /// A function path (the typed prefix already contains `::`).
    FunctionPath,
    /// A general value position: the right-hand side of a comparison or
    /// assignment, a `LET` value, or a call argument.
    Value,
    /// Nothing specific could be established. Completion still answers, with
    /// the union of everything nameable, rather than returning empty.
    Unknown,
}

/// Everything the candidate builder needs about the cursor position.
#[derive(Clone, Debug)]
pub struct CompletionContext {
    /// The classification.
    pub kind: ContextKind,
    /// The row tables in scope, innermost statement first.
    pub tables: Vec<String>,
    /// The kind expected at this position, when the position implies one.
    pub expected: Option<Kind>,
    /// The receiver's kind, for [`ContextKind::Member`]/
    /// [`ContextKind::Destructure`].
    pub receiver: Option<Kind>,
    /// The text already typed at the position (without a leading `$`).
    pub prefix: String,
    /// The byte range a completion item replaces.
    pub replace: (u32, u32),
    /// Names already written at this position (sibling destructure entries or
    /// object keys), which are not offered again.
    pub exclude: BTreeSet<String>,
    /// Whether the position accepts any completion at all. `false` inside a
    /// string or comment.
    pub enabled: bool,
}

impl CompletionContext {
    /// A context that offers nothing — the cursor is inside a string literal
    /// or a comment.
    fn disabled(offset: u32) -> Self {
        Self {
            kind: ContextKind::Unknown,
            tables: Vec::new(),
            expected: None,
            receiver: None,
            prefix: String::new(),
            replace: (offset, offset),
            exclude: BTreeSet::new(),
            enabled: false,
        }
    }
}

/// The in-scope variable kinds a position can see, keyed by name without the
/// leading `$`. Assembled by the caller from the analysis output (`LET`
/// bindings, inferred params), the schema (`DEFINE PARAM`), and the enclosing
/// DEFINE construct's context params.
pub(crate) type ParamKinds = BTreeMap<String, Kind>;

/// Classifies `offset` in `parsed`.
///
/// Never panics: every lookup is bounds-checked and every resolution failure
/// degrades to a weaker classification rather than an error.
pub(crate) fn classify(
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    params: &ParamKinds,
    offset: u32,
) -> CompletionContext {
    let source = parsed.text();
    let offset = offset.min(source.len() as u32);
    if in_ignored_region(parsed, offset) {
        return CompletionContext::disabled(offset);
    }

    let tokens = tokenize(source);
    let (cut, active) = locate(&tokens, offset);

    // A cursor inside a quoted run is inside a string literal: nothing to
    // complete, and offering identifiers there would be pure noise.
    if tokens.iter().any(|token| {
        token.kind == TokenKind::Quoted && token.start < offset && offset < token.end
    }) {
        return CompletionContext::disabled(offset);
    }

    let (prefix, replace) = prefix_at(&tokens, source, offset, active);
    let frames = frames(&tokens, source, cut);
    let env = Env {
        schema,
        params,
        tokens: &tokens,
        source,
    };

    let mut context = CompletionContext {
        kind: ContextKind::Unknown,
        tables: Vec::new(),
        expected: None,
        receiver: None,
        prefix,
        replace,
        exclude: BTreeSet::new(),
        enabled: true,
    };

    // Row tables: the innermost enclosing statement that names a target wins,
    // so a call argument inside a SELECT still sees the SELECT's table.
    context.tables = frames
        .iter()
        .enumerate()
        .rev()
        .find_map(|(level, frame)| {
            let nested = frames.len() - 1 - level;
            let (head, head_index, end) = statement_shape(&tokens, source, frame, cut, nested);
            head.and_then(|head| {
                let tables = env.target_tables(head, head_index, end);
                (!tables.is_empty()).then_some(tables)
            })
        })
        .unwrap_or_default();

    // --- 1. Member access and destructure: the most specific positions. ---
    if let Some(dot) = preceding_punct(&tokens, source, cut, ".") {
        if let Some(kind) = env.receiver_kind(dot, &context.tables) {
            context.kind = ContextKind::Member;
            context.receiver = Some(kind);
            return context;
        }
        // An unresolved receiver still means "a member goes here" — offering
        // tables or params instead would be wrong. Answer with nothing rather
        // than something misleading.
        context.kind = ContextKind::Member;
        return context;
    }
    if let Some(frame) = frames.last() {
        if frame.delimiter == b'{' {
            if let Some(open) = frame.open {
                if token_text(&tokens, source, open.wrapping_sub(1)) == Some(".") && open > 0 {
                    context.kind = ContextKind::Destructure;
                    context.receiver = env.receiver_kind(open - 1, &context.tables);
                    context.exclude = depth_zero_idents(&tokens, source, open + 1, cut);
                    return context;
                }
            }
        }
    }

    // --- 2. Param position: an active `$…` token. ---
    if active.is_some_and(|index| tokens[index].kind == TokenKind::Param) {
        context.kind = ContextKind::ParamName;
        context.expected = env.operand_expectation(cut, &context.tables);
        return context;
    }

    // --- 3. Function path: the typed prefix already carries a `::`. ---
    if context.prefix.contains("::") {
        context.kind = ContextKind::FunctionPath;
        context.expected = env.operand_expectation(cut, &context.tables);
        return context;
    }

    // --- 4. A graph step alternates edge, node, edge, node… ---
    if let Some(arrows) = graph_chain_arrows(&tokens, source, cut) {
        context.kind = if arrows % 2 == 1 {
            ContextKind::EdgeTable
        } else {
            ContextKind::TableName
        };
        return context;
    }

    // --- 5. Object literal keys and their values. ---
    if let Some(frame) = frames.last() {
        if frame.delimiter == b'{' && frame.is_object_literal {
            if let Some(open) = frame.open {
                context.exclude = object_keys(&tokens, source, open + 1, cut);
            }
            if let Some(colon) = preceding_punct(&tokens, source, cut, ":") {
                context.kind = ContextKind::Value;
                context.expected = token_text(&tokens, source, colon.wrapping_sub(1))
                    .filter(|_| colon > 0)
                    .and_then(|key| env.field_kind_on_tables(&context.tables, key));
                return context;
            }
            context.kind = ContextKind::ObjectKey;
            return context;
        }
    }

    // --- 6. Clause-driven classification. ---
    let frame = frames.last().copied().unwrap_or_default();
    context.kind = clause_position(&tokens, source, &frame, cut);
    if matches!(context.kind, ContextKind::Value | ContextKind::Unknown) {
        context.expected = env.operand_expectation(cut, &context.tables);
    }
    context
}

/// What the statement's shape says goes at the cursor.
///
/// The right-hand side of a comparison or assignment is a value position
/// whatever clause it sits in — that is what separates `WHERE status = ▏`
/// (a value) from `WHERE ▏` (a field). Otherwise the answer is the last clause
/// keyword's, falling back to the statement head's.
fn clause_position(tokens: &[Token], source: &str, frame: &Frame, cut: usize) -> ContextKind {
    if preceding_operator(tokens, source, cut).is_some_and(is_value_operator) {
        return ContextKind::Value;
    }
    let (head, head_index, _) = statement_shape(tokens, source, frame, cut, 0);
    let clause = last_clause(tokens, source, head_index, cut);

    match (head, clause) {
        // A table name follows the target keywords.
        (_, Some(Clause::From | Clause::Into | Clause::On | Clause::Only)) => ContextKind::TableName,
        (
            Some(
                Head::Create
                | Head::Update
                | Head::Upsert
                | Head::Delete
                | Head::Relate
                | Head::Insert
                | Head::Live,
            ),
            Some(Clause::Head),
        ) => ContextKind::TableName,
        // Field positions.
        (_, Some(Clause::Field)) => ContextKind::FieldName,
        (Some(Head::Select), Some(Clause::Head)) => ContextKind::FieldName,
        // Everything else that takes an expression.
        (_, Some(Clause::Value)) => ContextKind::Value,
        (Some(Head::Let | Head::Return | Head::If | Head::For | Head::Throw), _) => {
            ContextKind::Value
        }
        _ => ContextKind::Unknown,
    }
}

// ---------------------------------------------------------------------------
// Position
// ---------------------------------------------------------------------------

/// `(cut, active)`: the index of the first token at or after the cursor, and
/// the index of the completable token the cursor sits inside, if any.
fn locate(tokens: &[Token], offset: u32) -> (usize, Option<usize>) {
    let active = tokens.iter().position(|token| {
        token.start < offset
            && offset <= token.end
            && matches!(
                token.kind,
                TokenKind::Ident | TokenKind::Param | TokenKind::Number
            )
    });
    let cut = active.unwrap_or_else(|| {
        tokens
            .iter()
            .position(|token| token.start >= offset)
            .unwrap_or(tokens.len())
    });
    (cut, active)
}

/// The already-typed text and the range a completion item replaces. A `$`
/// prefix is stripped (items carry their own `$`), and the replaced range
/// always covers the whole token so accepting an item does not leave a tail.
fn prefix_at(
    tokens: &[Token],
    source: &str,
    offset: u32,
    active: Option<usize>,
) -> (String, (u32, u32)) {
    let Some(token) = active.and_then(|index| tokens.get(index)) else {
        return (String::new(), (offset, offset));
    };
    let typed = source
        .get(token.start as usize..offset as usize)
        .unwrap_or_default();
    let typed = typed.strip_prefix('$').unwrap_or(typed);
    (typed.to_string(), (token.start, token.end))
}

/// Whether `offset` sits inside a CST region where no identifier belongs — a
/// comment or a string literal. This is the one place the CST is consulted:
/// it recognizes SurrealQL's string flavours (`s"…"`, record-id strings,
/// format strings) without the token scan having to model each one.
fn in_ignored_region(parsed: &ParsedSource, offset: u32) -> bool {
    let root = parsed.tree().root_node();
    let mut node = root.descendant_for_byte_range(offset as usize, offset as usize);
    while let Some(current) = node {
        // Only a *strictly interior* position is inside the region; sitting at
        // its edge is a position next to it, where completion is fine.
        let interior = (current.start_byte() as u32) < offset && offset < (current.end_byte() as u32);
        if interior
            && matches!(
                current.kind(),
                "String" | "Comment" | "BlockComment" | "RecordIdString" | "FormatString"
            )
        {
            return true;
        }
        node = current.parent();
    }
    false
}

// ---------------------------------------------------------------------------
// Frames and statement bounds
// ---------------------------------------------------------------------------

/// One bracket nesting level, plus where the statement inside it begins.
#[derive(Clone, Copy, Debug, Default)]
struct Frame {
    /// The index of the opening delimiter token; `None` for the root frame.
    open: Option<usize>,
    /// `(`, `[`, `{`, or `0` for the root frame.
    delimiter: u8,
    /// The token index the innermost statement in this frame starts at.
    statement_start: usize,
    /// Whether a `{` frame is an object literal rather than a code block.
    is_object_literal: bool,
}

/// The frame stack at the cursor, outermost first. Unbalanced closers are
/// ignored rather than underflowing — broken input is the normal case here.
fn frames(tokens: &[Token], source: &str, cut: usize) -> Vec<Frame> {
    let mut stack = vec![Frame::default()];
    for (index, token) in tokens.iter().enumerate().take(cut) {
        if token.kind != TokenKind::Punct {
            continue;
        }
        match token.text(source) {
            "(" | "[" | "{" => {
                let delimiter = token.text(source).as_bytes()[0];
                stack.push(Frame {
                    open: Some(index),
                    delimiter,
                    statement_start: index + 1,
                    is_object_literal: delimiter == b'{'
                        && opens_an_object_literal(tokens, source, index),
                });
            }
            // An unbalanced closer would underflow to the root frame and lose
            // the statement start; broken input is the normal case here.
            ")" | "]" | "}" if stack.len() > 1 => {
                stack.pop();
            }
            ";" => {
                if let Some(frame) = stack.last_mut() {
                    frame.statement_start = index + 1;
                }
            }
            _ => {}
        }
    }
    stack
}

/// Walks `tokens[from..to]`, yielding each token with the bracket depth it
/// sits at relative to `from`.
///
/// Every structural scan below needs exactly this — "the depth-0 tokens of
/// this range, in order" — so the depth bookkeeping lives here once. A closing
/// delimiter is reported at the depth it closes *to*, so the `)` that ends the
/// range reads as depth 0.
fn scan<'a>(
    tokens: &'a [Token],
    source: &'a str,
    from: usize,
    to: usize,
) -> impl Iterator<Item = (usize, Token, i32)> + 'a {
    tokens
        .iter()
        .enumerate()
        .take(to)
        .skip(from)
        .scan(0i32, move |depth, (index, token)| {
            let mut level = *depth;
            if token.kind == TokenKind::Punct {
                match token.text(source) {
                    "(" | "[" | "{" => *depth += 1,
                    ")" | "]" | "}" => {
                        *depth -= 1;
                        level = *depth;
                    }
                    _ => {}
                }
            }
            Some((index, *token, level))
        })
}

/// Whether the `{` at `open` starts an object literal (`CONTENT { … }`,
/// `= { … }`) rather than a code block (`THEN { … }`, a function body). The
/// distinction decides whether the position inside wants object keys or
/// statements.
fn opens_an_object_literal(tokens: &[Token], source: &str, open: usize) -> bool {
    let Some(previous) = open.checked_sub(1).and_then(|index| tokens.get(index)) else {
        return false;
    };
    match previous.kind {
        TokenKind::Punct => matches!(previous.text(source), "=" | "," | "(" | "[" | ":"),
        TokenKind::Ident => matches!(
            previous.text(source).to_ascii_uppercase().as_str(),
            "CONTENT" | "MERGE" | "REPLACE" | "PATCH" | "SET" | "VALUES" | "RETURN"
        ),
        _ => false,
    }
}

/// `(head, head_index, statement_end)` for the statement of one frame.
///
/// The head is the *last* statement keyword at depth 0 before the cursor, so
/// `LET $rows = SELECT … WHERE ▏` classifies as a SELECT rather than a LET.
/// The end runs past the cursor to the next depth-0 `;`, the frame's close, or
/// end of input — that lookahead is what lets `SELECT ▏ FROM person` read its
/// own `FROM`.
///
/// `nested` is how many frames are open *inside* this one at the cursor. An
/// outer frame's statement does not end at the closer of an inner one, so the
/// forward scan starts that many levels deep — without it,
/// `SELECT array::len((▏)) FROM person` would decide the SELECT ends at the
/// first `)` and never see its own `FROM`.
fn statement_shape(
    tokens: &[Token],
    source: &str,
    frame: &Frame,
    cut: usize,
    nested: usize,
) -> (Option<Head>, usize, usize) {
    let start = frame.statement_start.min(tokens.len());
    let cut = cut.min(tokens.len());

    let mut head = None;
    let mut head_index = start;
    for (index, token, depth) in scan(tokens, source, start, cut) {
        if token.kind == TokenKind::Ident && depth == 0 {
            if let Some(found) = Head::from_text(token.text(source)) {
                head = Some(found);
                head_index = index;
            }
        }
    }

    let mut end = tokens.len();
    for (index, token, depth) in scan(tokens, source, cut, tokens.len()) {
        if token.kind != TokenKind::Punct {
            continue;
        }
        let text = token.text(source);
        // `depth` counts from the cursor, which sits `nested` levels inside
        // this frame. So this frame's own level is `-nested`: a `;` there ends
        // the statement, and a closer one level further out (`-nested - 1`)
        // closes the frame itself.
        let frame_level = -(nested as i32);
        if (matches!(text, ")" | "]" | "}") && depth == frame_level - 1)
            || (text == ";" && depth == frame_level)
        {
            end = index;
            break;
        }
    }
    (head, head_index, end)
}

/// The last clause keyword at depth 0 between the statement head and the
/// cursor.
fn last_clause(tokens: &[Token], source: &str, head_index: usize, cut: usize) -> Option<Clause> {
    let mut clause = None;
    for (_, token, depth) in scan(tokens, source, head_index, cut) {
        if token.kind == TokenKind::Ident && depth == 0 {
            if let Some(found) = Clause::from_text(token.text(source)) {
                clause = Some(found);
            }
        }
    }
    clause
}

/// The operator token immediately before the cursor, when one is there.
fn preceding_operator<'a>(tokens: &[Token], source: &'a str, cut: usize) -> Option<&'a str> {
    let token = tokens.get(cut.checked_sub(1)?)?;
    match token.kind {
        TokenKind::Punct => Some(token.text(source)),
        TokenKind::Ident => {
            let text = token.text(source);
            matches!(
                text.to_ascii_uppercase().as_str(),
                "CONTAINS" | "INSIDE" | "OUTSIDE" | "INTERSECTS" | "IS"
            )
            .then_some(text)
        }
        _ => None,
    }
}

/// How many arrows are in the graph-step chain ending at the cursor, or
/// `None` when the cursor is not in one.
///
/// A traversal alternates edge and node — `$a->works_at->company` — so the
/// arrow count's parity says which the cursor is at: odd is an edge slot,
/// even is the node it lands on.
fn graph_chain_arrows(tokens: &[Token], source: &str, cut: usize) -> Option<usize> {
    let mut index = cut.checked_sub(1)?;
    if !matches!(token_text(tokens, source, index), Some("->" | "<-")) {
        return None;
    }
    let mut arrows = 0usize;
    loop {
        match token_text(tokens, source, index) {
            Some("->" | "<-") => arrows += 1,
            _ => return Some(arrows),
        }
        // Step over the edge/node name before this arrow, if there is one.
        let Some(previous) = index.checked_sub(1) else {
            return Some(arrows);
        };
        if !matches!(
            tokens.get(previous).map(|token| token.kind),
            Some(TokenKind::Ident | TokenKind::Param)
        ) {
            return Some(arrows);
        }
        let Some(before) = previous.checked_sub(1) else {
            return Some(arrows);
        };
        index = before;
    }
}

/// The index of the punctuation token `symbol` immediately before the cursor.
fn preceding_punct(tokens: &[Token], source: &str, cut: usize, symbol: &str) -> Option<usize> {
    let index = cut.checked_sub(1)?;
    let token = tokens.get(index)?;
    (token.kind == TokenKind::Punct && token.text(source) == symbol).then_some(index)
}

fn token_text<'a>(tokens: &[Token], source: &'a str, index: usize) -> Option<&'a str> {
    tokens.get(index).map(|token| token.text(source))
}

/// Whether an operator puts the cursor on the value side of a comparison or
/// assignment, which is what turns a field position into a value position.
fn is_value_operator(operator: &str) -> bool {
    matches!(
        operator.to_ascii_uppercase().as_str(),
        "=" | "==" | "!=" | ">" | "<" | ">=" | "<=" | "+=" | "-=" | "*=" | "/=" | "?=" | "+?="
            | "~" | "!~" | "?~" | "*~" | "CONTAINS" | "INSIDE" | "OUTSIDE" | "INTERSECTS" | "IS"
    )
}

/// Identifier tokens at depth 0 in `[from, to)` — the sibling entries already
/// written in a destructure.
fn depth_zero_idents(
    tokens: &[Token],
    source: &str,
    from: usize,
    to: usize,
) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for (_, token, depth) in scan(tokens, source, from, to) {
        if token.kind == TokenKind::Ident && depth == 0 {
            names.insert(token.text(source).to_string());
        }
    }
    names
}

/// Keys already written in an object literal: the depth-0 identifiers that
/// are immediately followed by `:`.
fn object_keys(tokens: &[Token], source: &str, from: usize, to: usize) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for (index, token, depth) in scan(tokens, source, from, to) {
        if matches!(token.kind, TokenKind::Ident | TokenKind::Quoted)
            && depth == 0
            && token_text(tokens, source, index + 1) == Some(":")
        {
            names.insert(token.text(source).trim_matches(['"', '\'']).to_string());
        }
    }
    names
}

/// The `DEFINE` construct enclosing a position, recovered from tokens alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DefineShape {
    /// The word after `DEFINE`: `FIELD`, `EVENT`, `TABLE`, `ACCESS`, …
    pub keyword: String,
    /// The dotted name between the keyword and `ON` — a field path, for a
    /// `DEFINE FIELD`.
    pub name: String,
    /// The table named by `ON`, when the construct has one.
    pub table: Option<String>,
}

/// The `DEFINE` construct `offset` sits inside, or `None`.
///
/// Only the *outermost* statement is considered: a DEFINE is always top-level,
/// and its body's own statements must not be mistaken for it. This is the
/// fallback path for `context_params`, which cannot answer once a body stops
/// lowering — see [`crate::context_params::context_param_map_for`].
pub(crate) fn enclosing_define(source: &str, offset: u32) -> Option<DefineShape> {
    let tokens = tokenize(source);
    let (cut, _) = locate(&tokens, offset);
    let start = frames(&tokens, source, cut).first()?.statement_start;
    if !tokens
        .get(start)
        .is_some_and(|token| token.text(source).eq_ignore_ascii_case("DEFINE"))
    {
        return None;
    }
    let keyword = tokens.get(start + 1)?.text(source).to_string();

    let mut name = String::new();
    let mut table = None;
    let mut depth = 0i32;
    let mut index = start + 2;
    while index < tokens.len() {
        let token = tokens[index];
        let text = token.text(source);
        if token.kind == TokenKind::Punct {
            match text {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth -= 1,
                ";" if depth == 0 => break,
                "." if depth == 0 && table.is_none() => name.push('.'),
                _ => {}
            }
            index += 1;
            continue;
        }
        if depth != 0 {
            index += 1;
            continue;
        }
        if text.eq_ignore_ascii_case("ON") {
            // `ON [TABLE] <name>`.
            let mut next = index + 1;
            if tokens
                .get(next)
                .is_some_and(|token| token.text(source).eq_ignore_ascii_case("TABLE"))
            {
                next += 1;
            }
            table = tokens.get(next).map(|token| token.text(source).to_string());
            index = next + 1;
            continue;
        }
        // Anything that opens another clause ends the name.
        if table.is_none()
            && !matches!(
                text.to_ascii_uppercase().as_str(),
                "OVERWRITE" | "IF" | "NOT" | "EXISTS"
            )
            && Clause::from_text(text).is_none()
        {
            name.push_str(text);
        }
        index += 1;
    }
    Some(DefineShape {
        keyword,
        name,
        table,
    })
}

// ---------------------------------------------------------------------------
// Keyword vocabularies
// ---------------------------------------------------------------------------

/// A statement head keyword. Only the heads whose shape completion reads are
/// modeled; everything else classifies as `Unknown` and gets the fallback
/// candidate set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Head {
    Select,
    Create,
    Update,
    Upsert,
    Delete,
    Relate,
    Insert,
    Live,
    Define,
    Remove,
    Alter,
    Let,
    Return,
    If,
    For,
    Throw,
}

impl Head {
    fn from_text(text: &str) -> Option<Self> {
        Some(match text.to_ascii_uppercase().as_str() {
            "SELECT" => Self::Select,
            "CREATE" => Self::Create,
            "UPDATE" => Self::Update,
            "UPSERT" => Self::Upsert,
            "DELETE" => Self::Delete,
            "RELATE" => Self::Relate,
            "INSERT" => Self::Insert,
            "LIVE" => Self::Live,
            "DEFINE" => Self::Define,
            "REMOVE" => Self::Remove,
            "ALTER" => Self::Alter,
            "LET" => Self::Let,
            "RETURN" => Self::Return,
            "IF" => Self::If,
            "FOR" => Self::For,
            "THROW" => Self::Throw,
            _ => return None,
        })
    }
}

/// A clause keyword, collapsed to what it means for completion: the clause
/// that takes a table name, the one that takes field names, and so on. Two
/// keywords with the same completion meaning share a variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Clause {
    /// The head keyword itself is still the most recent clause.
    Head,
    /// `FROM` — a table position.
    From,
    /// `INTO` — a table position.
    Into,
    /// `ON` (`DEFINE FIELD … ON <table>`) — a table position.
    On,
    /// `ONLY` — still a table position.
    Only,
    /// A clause whose operands are field names.
    Field,
    /// A clause whose operand is an arbitrary value.
    Value,
}

impl Clause {
    fn from_text(text: &str) -> Option<Self> {
        Some(match text.to_ascii_uppercase().as_str() {
            "SELECT" | "CREATE" | "UPDATE" | "UPSERT" | "DELETE" | "RELATE" | "INSERT"
            | "LIVE" | "DEFINE" | "REMOVE" | "ALTER" | "LET" | "RETURN" | "IF" | "FOR"
            | "THROW" => Self::Head,
            "FROM" => Self::From,
            "INTO" => Self::Into,
            "ON" => Self::On,
            "ONLY" => Self::Only,
            "VALUE" | "OMIT" | "WHERE" | "SPLIT" | "GROUP" | "ORDER" | "BY" | "FETCH" | "SET"
            | "UNSET" | "FIELDS" | "COLUMNS" | "WHEN" | "ASSERT" => Self::Field,
            "LIMIT" | "START" | "TIMEOUT" | "CONTENT" | "MERGE" | "PATCH" | "REPLACE"
            | "DEFAULT" | "THEN" | "ELSE" | "IN" => Self::Value,
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// Resolution against the schema
// ---------------------------------------------------------------------------

/// Read-only view of everything resolution needs.
struct Env<'a> {
    schema: &'a SchemaIndex,
    params: &'a ParamKinds,
    tokens: &'a [Token],
    source: &'a str,
}

/// One step of a receiver idiom, walking left from a `.`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Segment {
    /// A `$name` root.
    Param(String),
    /// A named field or a root identifier.
    Name(String),
    /// A `[…]` subscript, which steps into a collection's element.
    Index,
}

impl Env<'_> {
    /// The tables a statement writes to or reads from, resolved to names that
    /// exist in the schema.
    fn target_tables(&self, head: Head, head_index: usize, end: usize) -> Vec<String> {
        let range = head_index.min(self.tokens.len())..end.min(self.tokens.len());
        let anchors: &[&str] = match head {
            Head::Select | Head::Live => &["FROM"],
            Head::Insert => &["INTO"],
            Head::Define | Head::Remove | Head::Alter => &["ON"],
            Head::Create | Head::Update | Head::Upsert | Head::Delete | Head::Relate => &[],
            _ => return Vec::new(),
        };

        // The token run that names the target: after the anchor keyword, or
        // straight after the head for the mutation statements.
        let mut start = None;
        if anchors.is_empty() {
            start = Some(head_index + 1);
        } else {
            for (index, token, depth) in scan(self.tokens, self.source, range.start, range.end) {
                if token.kind == TokenKind::Ident && depth == 0 {
                    let upper = token.text(self.source).to_ascii_uppercase();
                    if anchors.contains(&upper.as_str()) {
                        start = Some(index + 1);
                    }
                }
            }
        }

        let Some(mut index) = start else {
            return Vec::new();
        };
        let mut tables = Vec::new();
        // `RELATE a->edge->b` names its edge after the first arrow.
        if head == Head::Relate {
            while index < end && token_text(self.tokens, self.source, index) != Some("->") {
                index += 1;
            }
            index += 1;
        }
        while index < end.min(self.tokens.len()) {
            let token = self.tokens[index];
            let text = token.text(self.source);
            match token.kind {
                // Skip the modifiers that can sit between the keyword and the
                // target.
                TokenKind::Ident
                    if matches!(
                        text.to_ascii_uppercase().as_str(),
                        "ONLY" | "TABLE" | "IGNORE" | "RELATION" | "INDEX" | "FIELD" | "EVENT"
                    ) =>
                {
                    index += 1;
                    continue;
                }
                TokenKind::Ident if self.schema.table(text).is_some() => {
                    tables.push(text.to_string());
                }
                TokenKind::Param => {
                    let name = text.trim_start_matches('$');
                    if let Some(kind) = self.params.get(name) {
                        tables.extend(tables_of_kind(kind));
                    }
                }
                _ => {}
            }
            // Only a comma continues the target list.
            if token_text(self.tokens, self.source, index + 1) == Some(",") {
                index += 2;
                continue;
            }
            break;
        }
        tables.dedup();
        tables
    }

    /// The kind of the receiver whose `.` sits at `dot`.
    fn receiver_kind(&self, dot: usize, tables: &[String]) -> Option<Kind> {
        let segments = self.receiver_segments(dot)?;
        let (root, rest) = segments.split_first()?;
        let mut kind = match root {
            Segment::Param(name) => self.params.get(name).cloned()?,
            Segment::Name(name) => self.field_kind_on_tables(tables, name).or_else(|| {
                // A bare table name is a receiver too (`person.name`).
                self.schema
                    .table(name)
                    .map(|table| Kind::Record(vec![surrealdb_types::Table::from(table.name.as_str())]))
            })?,
            Segment::Index => return None,
        };
        for segment in rest {
            kind = match segment {
                Segment::Name(name) => step_field(&kind, name, self.schema)?,
                Segment::Index => {
                    crate::analyzer::expression::infer::collection_element_kind(&kind)?
                }
                Segment::Param(_) => return None,
            };
        }
        Some(kind)
    }

    /// The idiom immediately left of `dot`, as segments in source order.
    /// A `)` (a method call) or anything unrecognized yields `None` — a wrong
    /// receiver would produce confidently wrong members.
    fn receiver_segments(&self, dot: usize) -> Option<Vec<Segment>> {
        let mut segments = Vec::new();
        let mut index = dot;
        loop {
            let previous = index.checked_sub(1)?;
            let token = *self.tokens.get(previous)?;
            let text = token.text(self.source);
            match token.kind {
                TokenKind::Param => {
                    segments.push(Segment::Param(text.trim_start_matches('$').to_string()));
                    break;
                }
                TokenKind::Ident => {
                    if Head::from_text(text).is_some() || Clause::from_text(text).is_some() {
                        return None;
                    }
                    segments.push(Segment::Name(text.to_string()));
                    index = previous;
                }
                TokenKind::Punct if text == "]" => {
                    index = matching_open(self.tokens, self.source, previous)?;
                    segments.push(Segment::Index);
                }
                _ => return None,
            }
            if token_text(self.tokens, self.source, index.checked_sub(1)?) == Some(".") {
                index -= 1;
                continue;
            }
            break;
        }
        segments.reverse();
        (!segments.is_empty()).then_some(segments)
    }

    /// The declared kind of `field` on any of `tables` (including the
    /// implicit `id`/`in`/`out`).
    fn field_kind_on_tables(&self, tables: &[String], field: &str) -> Option<Kind> {
        tables.iter().find_map(|name| {
            let table = self.schema.table(name)?;
            crate::analyzer::data::select::kind_for_path(table, &[field.to_string()])
                .or_else(|| table.implicit_field_kind(field))
        })
    }

    /// The kind the position before the cursor expects.
    ///
    /// Two sources, checked in order: the left-hand side of a comparison or
    /// assignment right before the cursor (`WHERE status = ▏` expects
    /// `status`'s kind), and the parameter of an enclosing call at the
    /// cursor's argument index (`string::len(▏)` expects a `string`).
    fn operand_expectation(&self, cut: usize, tables: &[String]) -> Option<Kind> {
        if let Some(operator_index) = cut.checked_sub(1) {
            let token = *self.tokens.get(operator_index)?;
            let operator = token.text(self.source);
            if (token.kind == TokenKind::Punct || token.kind == TokenKind::Ident)
                && is_value_operator(operator)
            {
                if let Some(kind) = self.left_hand_kind(operator_index, tables) {
                    return Some(kind);
                }
            }
        }
        self.call_expectation(cut)
    }

    /// The kind of the idiom immediately left of the operator at `operator`.
    fn left_hand_kind(&self, operator: usize, tables: &[String]) -> Option<Kind> {
        let previous = operator.checked_sub(1)?;
        let token = *self.tokens.get(previous)?;
        match token.kind {
            TokenKind::Param => self
                .params
                .get(token.text(self.source).trim_start_matches('$'))
                .cloned(),
            TokenKind::Ident => {
                // A dotted idiom resolves through `receiver_kind`; a bare name
                // is a field of the row table.
                if token_text(self.tokens, self.source, previous + 1) == Some(".") {
                    return None;
                }
                if token_text(self.tokens, self.source, previous.wrapping_sub(1)) == Some(".")
                    && previous > 0
                {
                    let dot = previous - 1;
                    let base = self.receiver_kind(dot, tables)?;
                    return step_field(&base, token.text(self.source), self.schema);
                }
                self.field_kind_on_tables(tables, token.text(self.source))
            }
            _ => None,
        }
    }

    /// The declared kind of the enclosing call's parameter at the cursor's
    /// argument index. `fn::` functions resolve exactly, from their
    /// `DEFINE FUNCTION` signature; built-ins resolve from their documented
    /// parameter list, best-effort.
    fn call_expectation(&self, cut: usize) -> Option<Kind> {
        let open = enclosing_call(self.tokens, self.source, cut)?;
        let name = token_text(self.tokens, self.source, open.checked_sub(1)?)?;
        let argument = depth_zero_commas(self.tokens, self.source, open + 1, cut);
        if let Some(function) = self.schema.function(name) {
            return function.args.get(argument).and_then(|arg| arg.kind.clone());
        }
        let builtin = super::builtins::BUILTINS
            .iter()
            .find(|builtin| builtin.name == name)?;
        super::kind_text::parameter_kind(builtin.params, argument)
    }
}

/// The table names a kind refers to: a `record<a, b>` or `table<a>` under any
/// `option`/`array` wrapping.
fn tables_of_kind(kind: &Kind) -> Vec<String> {
    let (_, payload) = crate::kinds::peel_wrappers(kind);
    match payload {
        Kind::Record(tables) | Kind::Table(tables) => {
            tables.iter().map(ToString::to_string).collect()
        }
        _ => Vec::new(),
    }
}

/// `value.field`, stepping through record links, closed objects, and
/// collections, and preserving the `option`/`array` wrappers around them.
pub(crate) fn step_field(value: &Kind, field: &str, schema: &SchemaIndex) -> Option<Kind> {
    let (wrappers, payload) = crate::kinds::peel_wrappers(value);
    let stepped = match &payload {
        Kind::Record(targets) => {
            let mut resolved = Vec::new();
            for target in targets {
                let table = schema.table(&target.to_string())?;
                let kind = crate::analyzer::data::select::kind_for_path(
                    table,
                    &[field.to_string()],
                )
                .or_else(|| table.implicit_field_kind(field))?;
                resolved.push(kind);
            }
            (!resolved.is_empty()).then(|| Kind::either(resolved))?
        }
        _ => crate::analyzer::expression::infer::field_of_kind(&payload, field, schema)?,
    };
    Some(crate::kinds::rewrap_kind(&wrappers, stepped))
}

/// The index of the `(` that opens the call the cursor is inside, when the
/// innermost open paren is preceded by a name.
fn enclosing_call(tokens: &[Token], source: &str, cut: usize) -> Option<usize> {
    let mut stack = Vec::new();
    for (index, token) in tokens.iter().enumerate().take(cut) {
        if token.kind != TokenKind::Punct {
            continue;
        }
        match token.text(source) {
            "(" | "[" | "{" => stack.push(index),
            ")" | "]" | "}" => {
                stack.pop();
            }
            _ => {}
        }
    }
    let open = *stack.last()?;
    (tokens.get(open)?.text(source) == "("
        && tokens.get(open.checked_sub(1)?)?.kind == TokenKind::Ident)
        .then_some(open)
}

/// The number of argument separators at depth 0 in `[from, to)` — the index
/// of the argument the cursor is in.
fn depth_zero_commas(tokens: &[Token], source: &str, from: usize, to: usize) -> usize {
    scan(tokens, source, from, to)
        .filter(|(_, token, depth)| {
            *depth == 0 && token.kind == TokenKind::Punct && token.text(source) == ","
        })
        .count()
}

/// The index of the delimiter opening the group that `close` ends.
fn matching_open(tokens: &[Token], source: &str, close: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (index, token) in tokens.iter().enumerate().take(close).rev() {
        if token.kind != TokenKind::Punct {
            continue;
        }
        match token.text(source) {
            ")" | "]" | "}" => depth += 1,
            "(" | "[" | "{" => {
                if depth == 0 {
                    return Some(index);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}
