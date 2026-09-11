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

use crate::text::LineIndex;

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

/// Delta-encodes tokens against the document `lines` indexes. Tokens must be
/// sorted and non-overlapping; ones that span a line are split, since the
/// protocol has no way to express a multi-line token.
///
/// Taking the index rather than the text is deliberate: an `offset_to_position`
/// per token would rescan the document per token and go quadratic in file
/// size, and this runs after every edit.
pub fn encode(lines: &LineIndex, tokens: &[Token]) -> Vec<SemanticToken> {
    let mut encoded = Vec::with_capacity(tokens.len());
    let (mut last_line, mut last_start) = (0u32, 0u32);

    for token in tokens {
        for (line, start, length) in split(lines, token.range.clone()) {
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

/// Splits a byte range into per-line `(line, utf16 start column, utf16
/// length)` pieces. A range that stays on one line yields one piece.
///
/// A trailing newline never belongs to a token, so a piece that would end on
/// one stops short of it.
fn split(lines: &LineIndex, range: Range<usize>) -> impl Iterator<Item = (u32, u32, u32)> + use<> {
    let text = lines.text();
    let first = lines.line_of(range.start.min(text.len()));
    let last = lines.line_of(range.end.max(range.start).min(text.len()));
    let mut pieces = Vec::with_capacity(last - first + 1);
    for line in first..=last {
        let start = range.start.max(lines.line_start(line));
        let mut end = range.end.min(lines.line_end(line));
        if text[..end.min(text.len())].ends_with('\n') {
            end -= 1;
        }
        if end <= start {
            continue;
        }
        let column = lines.column(start);
        pieces.push((line as u32, column, lines.column(end) - column));
    }
    pieces.into_iter()
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

    /// Tokenizes and encodes `text`, the pair every test here wants.
    fn encoded(text: &str) -> Vec<SemanticToken> {
        encode(&LineIndex::for_str(text), &tokens(&parse(text)))
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
        let encoded = encoded(text);
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
        let encoded = encoded(text);
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
        let encoded = encoded(text);
        let string = encoded
            .iter()
            .find(|token| token.token_type == TokenKind::String.index())
            .expect("the string is tokenized");
        assert_eq!((string.delta_start, string.length), (7, 5));
    }
}
