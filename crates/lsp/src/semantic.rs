//! Semantic tokens for SurrealQL.
//!
//! The classification itself lives in [`surrealguard_syntax::highlight`],
//! shared with the TypeScript language-service plugin so an embedded query
//! gets the same answer whichever surface asks. What is here is the part that
//! is LSP: the legend, the mapping of a query's tokens onto its host file, and
//! the delta encoding.
//!
//! Two protocol constraints shape the encoder: a token may not span a line
//! (multi-line strings and comments are split per line), and tokens are
//! delta-encoded against their predecessor, so they must be emitted in
//! ascending position order.

use std::ops::Range;

use tower_lsp::lsp_types::{SemanticToken, SemanticTokenType};

pub use surrealguard_syntax::highlight::{tokens, Token};

/// The legend, in index order — a token's `token_type` is
/// [`TokenKind::index`], so this must stay in the enum's variant order.
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
                token_type: token.kind.index(),
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
    use surrealguard_syntax::highlight::TokenKind;
    use surrealguard_syntax::parse::{parse_source, ParsedSource};
    use surrealguard_syntax::source::SourceId;

    fn parse(text: &str) -> ParsedSource {
        parse_source(SourceId::new("t.surql"), text).expect("parses")
    }

    #[test]
    fn the_legend_matches_the_classifier_order() {
        // The client is handed TOKEN_TYPES and then handed indices produced by
        // `TokenKind::index`. If the two orders ever drift, every token is
        // painted as some other kind and nothing fails loudly.
        for (index, kind) in [
            TokenKind::Keyword,
            TokenKind::Comment,
            TokenKind::String,
            TokenKind::Number,
            TokenKind::Regexp,
            TokenKind::Operator,
            TokenKind::Type,
            TokenKind::Function,
            TokenKind::Variable,
            TokenKind::Parameter,
            TokenKind::Property,
            TokenKind::EnumMember,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(kind.index() as usize, index);
            assert_eq!(TOKEN_TYPES[index].as_str(), kind.lsp_name());
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
            .filter(|token| token.token_type == TokenKind::String.index())
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
            .find(|token| token.token_type == TokenKind::String.index())
            .expect("the string is tokenized");
        assert_eq!((string.delta_start, string.length), (7, 5));
    }
}
