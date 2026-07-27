//! Semantic tokens for SurrealQL.
//!
//! Tree-sitter already tells the editor what every byte of a `.surql` file
//! is — but only where the editor has the SurrealQL grammar. Inside a
//! `` surql`…` `` template in a `.svelte` or `.ts` file the host grammar sees
//! one long string, and no amount of extension configuration changes that:
//! the injection would have to be declared by the *host* language. Semantic
//! tokens are the protocol's answer. The server, which does have the grammar,
//! reports token kinds at host coordinates and the editor paints them.
//!
//! The kind mapping mirrors the `highlights.scm` the Zed extension ships, so
//! an embedded query and a `.surql` file agree about what a keyword is.
//!
//! Two protocol constraints shape this module: a token may not span a line
//! (multi-line strings and comments are split per line), and tokens are
//! delta-encoded against their predecessor, so they must be emitted in
//! ascending position order.

use std::ops::Range;

use surrealguard_syntax::parse::ParsedSource;
use tower_lsp::lsp_types::{SemanticToken, SemanticTokenType};
use tree_sitter::Node;

/// The legend, in index order — a token's `token_type` is its position here.
pub const TOKEN_TYPES: [SemanticTokenType; 12] = [
    SemanticTokenType::KEYWORD,
    SemanticTokenType::COMMENT,
    SemanticTokenType::STRING,
    SemanticTokenType::NUMBER,
    SemanticTokenType::REGEXP,
    SemanticTokenType::OPERATOR,
    SemanticTokenType::TYPE,
    SemanticTokenType::FUNCTION,
    SemanticTokenType::VARIABLE,
    SemanticTokenType::PARAMETER,
    SemanticTokenType::PROPERTY,
    SemanticTokenType::ENUM_MEMBER,
];

const KEYWORD: u32 = 0;
const COMMENT: u32 = 1;
const STRING: u32 = 2;
const NUMBER: u32 = 3;
const REGEXP: u32 = 4;
const OPERATOR: u32 = 5;
const TYPE: u32 = 6;
const FUNCTION: u32 = 7;
const VARIABLE: u32 = 8;
const PARAMETER: u32 = 9;
const PROPERTY: u32 = 10;
const ENUM_MEMBER: u32 = 11;

/// One token as a byte range in some text, before delta encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Byte range in the text the token was found in.
    pub range: Range<usize>,
    /// Index into [`TOKEN_TYPES`].
    pub kind: u32,
}

/// Every semantic token in a parsed SurrealQL source, in ascending order.
pub fn tokens(parsed: &ParsedSource) -> Vec<Token> {
    let mut tokens = Vec::new();
    collect(parsed.tree().root_node(), &mut tokens);
    tokens
}

/// Walks the tree, emitting the outermost node that *is* a token. Descending
/// past one would emit overlapping tokens, which the protocol forbids.
fn collect(node: Node<'_>, tokens: &mut Vec<Token>) {
    if let Some(kind) = token_kind(node) {
        tokens.push(Token {
            range: node.byte_range(),
            kind,
        });
        return;
    }
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();
    for child in children {
        collect(child, tokens);
    }
}

/// The token kind for a grammar node, or `None` when the node is structure
/// rather than a token (descend into it) — or punctuation, which carries no
/// meaning the editor cannot see for itself.
fn token_kind(node: Node<'_>) -> Option<u32> {
    if !node.is_named() {
        return None;
    }
    Some(match node.kind() {
        "Comment" | "BlockComment" => COMMENT,
        // `true`, `NONE`, `NULL`: keyword-shaped literals.
        "Keyword" | "Bool" | "None" | "Literal" => KEYWORD,
        // Enumerated constants a clause accepts by name.
        "Distance" | "Filter" | "AnalyzerTokenizer" | "TokenType" | "HttpMethod" => ENUM_MEMBER,
        "Number" | "Int" | "Float" | "Decimal" | "Duration" | "DurationPart" | "DurationValue"
        | "VersionNumber" => NUMBER,
        "String" | "FormatString" | "RecordIdString" => STRING,
        "Regex" => REGEXP,
        "TypeName" => TYPE,
        "FunctionName" | "IdiomFunction" => FUNCTION,
        // `$param` — in SurrealQL a parameter is exactly what it looks like.
        "VariableName" => PARAMETER,
        "KeyName" | "ObjectKey" => PROPERTY,
        "RecordTbIdent" => TYPE,
        "RecordIdIdent" => ENUM_MEMBER,
        "Operator" | "RangeOp" | "LookupLeft" | "LookupRight" | "LookupBoth" | "Any" | "At"
        | "Optional" | "Pipe" => OPERATOR,
        // A bare name is a variable; the same name reached through a path is
        // a field of whatever the path walked into.
        "Ident" => {
            if node
                .parent()
                .is_some_and(|parent| matches!(parent.kind(), "Path" | "Subscript" | "Lookup"))
            {
                PROPERTY
            } else {
                VARIABLE
            }
        }
        _ => return None,
    })
}

/// Re-expresses an embedded query's tokens as ranges in its host file.
///
/// A token is kept only when it maps to a contiguous run of host bytes. One
/// that straddles a `${...}` substitution does not: it covers a generated
/// `$__hostN` name on our side and an arbitrary host expression on the
/// editor's, and painting it would colour the wrong text.
pub fn map_to_host(query: &surrealguard_embed::EmbeddedQuery, tokens: Vec<Token>) -> Vec<Token> {
    tokens
        .into_iter()
        .filter_map(|token| {
            let length = token.range.end - token.range.start;
            let host = query.host_span(token.range);
            (host.end - host.start == length).then_some(Token {
                range: host,
                kind: token.kind,
            })
        })
        .collect()
}

/// Delta-encodes tokens against `text`. Tokens must be sorted and
/// non-overlapping; ones that span a line are split, since the protocol has no
/// way to express a multi-line token.
///
/// The single pass over `text` is deliberate: an `offset_to_position` per
/// token would be quadratic in file size, and this runs after every edit.
pub fn encode(text: &str, tokens: &[Token]) -> Vec<SemanticToken> {
    let mut lines = LineIndex::new(text);
    let mut encoded = Vec::with_capacity(tokens.len());
    let (mut last_line, mut last_start) = (0u32, 0u32);

    for token in tokens {
        for (line, start, length) in lines.split(text, token.range.clone()) {
            if length == 0 {
                continue;
            }
            let delta_line = line - last_line;
            let delta_start = if delta_line == 0 {
                start.saturating_sub(last_start)
            } else {
                start
            };
            encoded.push(SemanticToken {
                delta_line,
                delta_start,
                length,
                token_type: token.kind,
                token_modifiers_bitset: 0,
            });
            (last_line, last_start) = (line, start);
        }
    }
    encoded
}

/// A forward-only cursor over a text's lines, so encoding a whole document's
/// tokens costs one pass over it rather than one per token.
struct LineIndex {
    /// Byte offset of each line's first byte.
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(text: &str) -> Self {
        let mut starts = vec![0usize];
        starts.extend(
            text.match_indices('\n')
                .map(|(offset, newline)| offset + newline.len()),
        );
        Self { starts }
    }

    /// The line containing `offset`.
    fn line_of(&self, offset: usize) -> usize {
        self.starts.partition_point(|start| *start <= offset) - 1
    }

    /// Splits a byte range into per-line `(line, utf16 start column, utf16
    /// length)` pieces. A range that stays on one line yields one piece.
    fn split(
        &mut self,
        text: &str,
        range: Range<usize>,
    ) -> impl Iterator<Item = (u32, u32, u32)> + use<> {
        let first = self.line_of(range.start);
        let last = self.line_of(range.end.max(range.start));
        let mut pieces = Vec::with_capacity(last - first + 1);
        for line in first..=last {
            let line_start = self.starts[line];
            let line_end = self
                .starts
                .get(line + 1)
                .map_or(text.len(), |next| *next)
                .min(text.len());
            let start = range.start.max(line_start);
            let end = range.end.min(line_end);
            if end <= start {
                continue;
            }
            let column = utf16_len(&text[line_start..start]);
            let length = utf16_len(&text[start..end.min(text.len())]);
            pieces.push((line as u32, column, length));
        }
        pieces.into_iter()
    }
}

/// A string's length in UTF-16 code units — the protocol's default unit for
/// both the column and the token length. A trailing newline never belongs to
/// a token, so it is not counted.
fn utf16_len(text: &str) -> u32 {
    text.trim_end_matches('\n')
        .chars()
        .map(|ch| ch.len_utf16() as u32)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    fn parse(text: &str) -> ParsedSource {
        parse_source(SourceId::new("t.surql"), text).expect("parses")
    }

    /// `(covered text, token type)` for each token, which is what actually
    /// matters: the ranges have to land on the right bytes.
    fn described(text: &str) -> Vec<String> {
        tokens(&parse(text))
            .into_iter()
            .map(|token| {
                format!(
                    "{} {}",
                    TOKEN_TYPES[token.kind as usize].as_str(),
                    &text[token.range]
                )
            })
            .collect()
    }

    #[test]
    fn a_select_is_tokenized_by_role() {
        let described = described("SELECT name FROM person WHERE age > 21;");
        assert_eq!(
            described,
            [
                "keyword SELECT",
                "variable name",
                "keyword FROM",
                "variable person",
                "keyword WHERE",
                "variable age",
                "operator >",
                "number 21",
            ]
        );
    }

    #[test]
    fn parameters_strings_and_comments_are_distinguished() {
        let described = described("-- note\nRETURN $name = 'ada';");
        assert_eq!(
            described,
            [
                "comment -- note",
                "keyword RETURN",
                "parameter $name",
                "operator =",
                "string 'ada'",
            ]
        );
    }

    #[test]
    fn tokens_never_overlap_and_always_advance() {
        let text = "DEFINE TABLE person SCHEMAFULL;\n\
                    DEFINE FIELD name ON person TYPE string;\n\
                    SELECT name, math::sum(scores) FROM person:ada;";
        let tokens = tokens(&parse(text));
        assert!(tokens.len() > 10, "the corpus should produce real tokens");
        for pair in tokens.windows(2) {
            assert!(
                pair[0].range.end <= pair[1].range.start,
                "tokens must be ordered and disjoint: {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn encoding_is_relative_to_the_previous_token() {
        let text = "SELECT name\nFROM person;";
        let encoded = encode(text, &tokens(&parse(text)));
        // SELECT at 0:0, name at 0:7 (delta 7), FROM on the next line at
        // column 0, person 5 further along.
        assert_eq!(
            encoded
                .iter()
                .map(|t| (t.delta_line, t.delta_start, t.length))
                .collect::<Vec<_>>(),
            vec![(0, 0, 6), (0, 7, 4), (1, 0, 4), (0, 5, 6)]
        );
    }

    #[test]
    fn a_token_that_spans_lines_is_split_at_the_newline() {
        // The protocol has no multi-line token; a string that wraps must come
        // back as one piece per line or the editor paints from the wrong
        // column to the end of the file.
        let text = "RETURN 'one\ntwo';";
        let encoded = encode(text, &tokens(&parse(text)));
        let string_pieces: Vec<_> = encoded
            .iter()
            .filter(|token| token.token_type == STRING)
            .map(|token| (token.delta_line, token.delta_start, token.length))
            .collect();
        assert_eq!(
            string_pieces,
            vec![(0, 7, 4), (1, 0, 4)],
            "`'one` on the first line, `two'` on the second"
        );
    }

    #[test]
    fn columns_and_lengths_are_utf16_code_units() {
        // 'é' is two UTF-8 bytes and one UTF-16 unit; an emoji is four bytes
        // and two units. A byte-counting encoder puts every later token on
        // this line in the wrong place.
        let text = "RETURN 'é😀';";
        let encoded = encode(text, &tokens(&parse(text)));
        let string = encoded
            .iter()
            .find(|token| token.token_type == STRING)
            .expect("the string is tokenized");
        assert_eq!((string.delta_start, string.length), (7, 5));
    }
}
