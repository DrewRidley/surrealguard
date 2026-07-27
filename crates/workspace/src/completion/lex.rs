//! A comment-skipping SurrealQL token scan, used by completion to classify a
//! cursor position.
//!
//! Completion fires where the text does *not* parse — `SELECT name FROM ▏`,
//! `WHERE status = $▏` — so the CST alone cannot be the backbone: tree-sitter
//! recovery sometimes collapses a whole statement into one `ERROR` node, which
//! erases exactly the structure completion needs. A token scan degrades
//! smoothly instead: an unfinished statement is still a keyword followed by
//! some tokens, and that is all the context classifier reads.
//!
//! The scan is deliberately coarse. It does not distinguish keywords from
//! identifiers (the classifier does, by matching text), and it merges a
//! `::`-joined function path into a single token so `string::le▏` is one
//! prefix rather than three.

/// What a token is, at the granularity completion needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TokenKind {
    /// An identifier, a keyword, or a `::`-joined function path (including a
    /// trailing `::`, so `string::` is one token).
    Ident,
    /// A `$name` parameter. A lone `$` is a `Param` with an empty name.
    Param,
    /// A numeric or duration literal.
    Number,
    /// A quoted run: `'…'`, `"…"`, `` `…` ``, or `⟨…⟩`.
    Quoted,
    /// Anything else: one operator or delimiter.
    Punct,
}

/// One token: what it is and the byte range it covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Token {
    /// The token's class.
    pub kind: TokenKind,
    /// Byte offset of the first character.
    pub start: u32,
    /// Byte offset one past the last character.
    pub end: u32,
}

impl Token {
    /// The token's source text.
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        source
            .get(self.start as usize..self.end as usize)
            .unwrap_or_default()
    }
}

/// Operators worth keeping whole, longest first — the classifier tests for
/// `->`/`<-` (graph steps) and `..` (ranges), and splitting them would make a
/// `<` look like a comparison.
const OPERATORS: &[&str] = &[
    "<->", "...", "+?=", "->", "<-", "..", "==", "!=", ">=", "<=", "&&", "||", "??", "?:", "+=",
    "-=", "*=", "/=", "?=", "*=", "?~", "*~", "<|", "|>", "@@",
];

/// Tokenizes `source`, dropping whitespace and comments.
///
/// Never panics and never splits a UTF-8 character: every byte at or above
/// `0x80` (a multi-byte lead or continuation byte) is treated as an
/// identifier character, so a cut can only land on an ASCII boundary.
pub(crate) fn tokenize(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        let byte = bytes[i];

        if byte.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // Comments: `--`, `//`, `#` to end of line; `/* … */` (unterminated
        // runs to end of input, which is what an in-progress comment is).
        if byte == b'#'
            || (byte == b'-' && bytes.get(i + 1) == Some(&b'-'))
            || (byte == b'/' && bytes.get(i + 1) == Some(&b'/'))
        {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if byte == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }

        let start = i;

        if byte == b'$' {
            i += 1;
            while i < bytes.len() && is_ident_continue(bytes[i]) {
                i += 1;
            }
            tokens.push(token(TokenKind::Param, start, i));
            continue;
        }

        if is_ident_start(byte) {
            i = scan_ident_path(bytes, i);
            tokens.push(token(TokenKind::Ident, start, i));
            continue;
        }

        if byte.is_ascii_digit() {
            i += 1;
            while i < bytes.len()
                && (is_ident_continue(bytes[i])
                    || (bytes[i] == b'.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)))
            {
                i += 1;
            }
            tokens.push(token(TokenKind::Number, start, i));
            continue;
        }

        if let Some(end) = scan_quoted(bytes, i) {
            tokens.push(token(TokenKind::Quoted, start, end));
            i = end;
            continue;
        }

        let rest = &source[i..];
        let operator = OPERATORS
            .iter()
            .find(|operator| rest.starts_with(**operator))
            .copied();
        i += operator.map_or_else(|| char_len(bytes, i), |operator: &str| operator.len());
        tokens.push(token(TokenKind::Punct, start, i));
    }

    tokens
}

/// Scans an identifier and any `::`-joined continuation, including a trailing
/// `::` with nothing after it — `string::` is a single in-progress path token,
/// which is what makes function-path completion a plain prefix match.
fn scan_ident_path(bytes: &[u8], from: usize) -> usize {
    let mut i = from;
    loop {
        while i < bytes.len() && is_ident_continue(bytes[i]) {
            i += 1;
        }
        if bytes.get(i) == Some(&b':') && bytes.get(i + 1) == Some(&b':') {
            i += 2;
            continue;
        }
        return i;
    }
}

/// The end of the quoted run starting at `from`, or `None` when `from` does
/// not open one. An unterminated run ends at end of input (the common
/// in-progress case).
fn scan_quoted(bytes: &[u8], from: usize) -> Option<usize> {
    let (close, escapes): (&[u8], bool) = match bytes[from] {
        b'\'' => (b"'", true),
        b'"' => (b"\"", true),
        b'`' => (b"`", false),
        // `⟨` is `E2 9F A8`; its close `⟩` is `E2 9F A9`.
        0xE2 if bytes[from..].starts_with(&[0xE2, 0x9F, 0xA8]) => (&[0xE2, 0x9F, 0xA9], false),
        _ => return None,
    };
    let open_len = if close.len() == 3 { 3 } else { 1 };
    let mut i = from + open_len;
    while i < bytes.len() {
        if escapes && bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i..].starts_with(close) {
            return Some(i + close.len());
        }
        i += 1;
    }
    Some(bytes.len())
}

fn token(kind: TokenKind, start: usize, end: usize) -> Token {
    Token {
        kind,
        start: start.min(u32::MAX as usize) as u32,
        end: end.min(u32::MAX as usize) as u32,
    }
}

/// The byte length of the character starting at `at`, so an unrecognized
/// non-ASCII byte still advances by a whole character.
fn char_len(bytes: &[u8], at: usize) -> usize {
    match bytes[at] {
        b if b < 0x80 => 1,
        b if b >> 5 == 0b110 => 2,
        b if b >> 4 == 0b1110 => 3,
        _ => 4,
    }
    .min(bytes.len() - at)
    .max(1)
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80
}

fn is_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<(TokenKind, &str)> {
        tokenize(source)
            .into_iter()
            .map(|token| (token.kind, token.text(source)))
            .collect()
    }

    #[test]
    fn a_select_tokenizes_into_keywords_idents_and_operators() {
        assert_eq!(
            kinds("SELECT name FROM person WHERE age > 3"),
            vec![
                (TokenKind::Ident, "SELECT"),
                (TokenKind::Ident, "name"),
                (TokenKind::Ident, "FROM"),
                (TokenKind::Ident, "person"),
                (TokenKind::Ident, "WHERE"),
                (TokenKind::Ident, "age"),
                (TokenKind::Punct, ">"),
                (TokenKind::Number, "3"),
            ]
        );
    }

    #[test]
    fn a_function_path_is_one_token_even_while_being_typed() {
        assert_eq!(kinds("string::"), vec![(TokenKind::Ident, "string::")]);
        assert_eq!(
            kinds("fn::organization::modules($o)"),
            vec![
                (TokenKind::Ident, "fn::organization::modules"),
                (TokenKind::Punct, "("),
                (TokenKind::Param, "$o"),
                (TokenKind::Punct, ")"),
            ]
        );
    }

    #[test]
    fn a_lone_dollar_is_a_param_with_an_empty_name() {
        assert_eq!(
            kinds("WHERE status = $"),
            vec![
                (TokenKind::Ident, "WHERE"),
                (TokenKind::Ident, "status"),
                (TokenKind::Punct, "="),
                (TokenKind::Param, "$"),
            ]
        );
    }

    #[test]
    fn comments_are_dropped_and_never_swallow_the_next_statement() {
        assert_eq!(
            kinds("-- note\nSELECT 1; /* block */ SELECT 2; # tail"),
            vec![
                (TokenKind::Ident, "SELECT"),
                (TokenKind::Number, "1"),
                (TokenKind::Punct, ";"),
                (TokenKind::Ident, "SELECT"),
                (TokenKind::Number, "2"),
                (TokenKind::Punct, ";"),
            ]
        );
    }

    #[test]
    fn graph_steps_and_ranges_stay_whole() {
        assert_eq!(
            kinds("$a->employee_of->$b"),
            vec![
                (TokenKind::Param, "$a"),
                (TokenKind::Punct, "->"),
                (TokenKind::Ident, "employee_of"),
                (TokenKind::Punct, "->"),
                (TokenKind::Param, "$b"),
            ]
        );
        assert_eq!(kinds("1..3")[1], (TokenKind::Punct, ".."));
    }

    #[test]
    fn strings_hold_their_content_and_an_unterminated_one_ends_at_eof() {
        assert_eq!(
            kinds("a = 'x FROM y'"),
            vec![
                (TokenKind::Ident, "a"),
                (TokenKind::Punct, "="),
                (TokenKind::Quoted, "'x FROM y'"),
            ]
        );
        assert_eq!(
            kinds("'unterminated"),
            vec![(TokenKind::Quoted, "'unterminated")]
        );
    }

    #[test]
    fn multibyte_text_never_splits_a_character() {
        // Every token boundary must be a char boundary, or `text()` returns
        // nothing and the classifier silently loses the token.
        let source = "SELECT ⟨naïve⟩, 'héllo' FROM café";
        for token in tokenize(source) {
            assert!(source.is_char_boundary(token.start as usize));
            assert!(source.is_char_boundary(token.end as usize));
            assert!(!token.text(source).is_empty());
        }
    }

    #[test]
    fn tokenizing_arbitrary_broken_input_never_panics() {
        for source in [
            "",
            "$",
            ".",
            "::",
            "SELECT * FROM",
            "{{{{",
            "'",
            "/*",
            "⟨",
            "SELECT .{ FROM",
            "1.",
        ] {
            let _ = tokenize(source);
        }
    }
}
