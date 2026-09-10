//! What every byte of a SurrealQL source *is*, for a highlighter.
//!
//! Tree-sitter already knows — but only where the editor has the SurrealQL
//! grammar. Inside a `db.query("…")` literal in a `.svelte` or `.ts` file the
//! host grammar sees one long string, and no amount of extension
//! configuration changes that: the injection would have to be declared by the
//! *host* language. So the classification has to come from us, and it has to
//! come from here rather than from any one editor surface: the LSP paints it
//! as semantic tokens, and the TypeScript language-service plugin paints the
//! same bytes as TypeScript's own classifications. Two surfaces, one answer
//! about what a keyword is.
//!
//! This module is the classification and nothing else — no line splitting, no
//! delta encoding, no protocol. Those belong to whoever is being spoken to.

use std::ops::Range;

use tree_sitter::Node;

use crate::parse::ParsedSource;

/// What a token is, in the vocabulary both highlighting surfaces share.
///
/// The variant order is the LSP legend order, and [`TokenKind::index`] is that
/// legend index — changing it renumbers a wire format, so append rather than
/// reorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TokenKind {
    /// A language keyword: `SELECT`, `FROM`, and the keyword-shaped literals
    /// (`true`, `NONE`, `NULL`).
    Keyword,
    /// A line or block comment.
    Comment,
    /// A string literal, including format and record-id strings.
    String,
    /// Any numeric literal, plus durations and version numbers.
    Number,
    /// A regular-expression literal.
    Regexp,
    /// An operator or a punctuation-shaped connective that carries meaning.
    Operator,
    /// A type name (`string`, `option<…>`) or the table half of a record id.
    Type,
    /// A function name, whether a path (`math::sum`) or a method call.
    Function,
    /// A bare name standing on its own.
    Variable,
    /// A `$param`.
    Parameter,
    /// A field or object key — a name reached *through* something.
    Property,
    /// An enumerated constant a clause accepts by name, and the id half of a
    /// record id.
    EnumMember,
}

impl TokenKind {
    /// The kind's index in the shared legend order.
    #[must_use]
    pub fn index(self) -> u32 {
        self as u32
    }

    /// The kind's LSP semantic-token type name — the legend entry a client is
    /// told to expect at [`TokenKind::index`].
    #[must_use]
    pub fn lsp_name(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::Comment => "comment",
            Self::String => "string",
            Self::Number => "number",
            Self::Regexp => "regexp",
            Self::Operator => "operator",
            Self::Type => "type",
            Self::Function => "function",
            Self::Variable => "variable",
            Self::Parameter => "parameter",
            Self::Property => "property",
            Self::EnumMember => "enumMember",
        }
    }
}

/// One token as a byte range in some text, before any encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Byte range in the text the token was found in.
    pub range: Range<usize>,
    /// What the token is.
    pub kind: TokenKind,
}

/// Every token in a parsed SurrealQL source, in ascending order, disjoint.
#[must_use]
pub fn tokens(parsed: &ParsedSource) -> Vec<Token> {
    let mut tokens = Vec::new();
    collect(parsed.tree().root_node(), &mut tokens);
    tokens
}

/// Walks the tree, emitting the outermost node that *is* a token. Descending
/// past one would emit overlapping tokens, which every consumer forbids.
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
fn token_kind(node: Node<'_>) -> Option<TokenKind> {
    use TokenKind::{
        Comment, EnumMember, Function, Keyword, Number, Operator, Parameter, Property, Regexp,
        String as Str, Type, Variable,
    };
    if !node.is_named() {
        return None;
    }
    Some(match node.kind() {
        "Comment" | "BlockComment" => Comment,
        // `true`, `NONE`, `NULL`: keyword-shaped literals.
        "Keyword" | "Bool" | "None" | "Literal" => Keyword,
        // Enumerated constants a clause accepts by name.
        "Distance" | "Filter" | "AnalyzerTokenizer" | "TokenType" | "HttpMethod" => EnumMember,
        "Number" | "Int" | "Float" | "Decimal" | "Duration" | "DurationPart" | "DurationValue"
        | "VersionNumber" => Number,
        "String" | "FormatString" | "RecordIdString" => Str,
        "Regex" => Regexp,
        "TypeName" => Type,
        "FunctionName" | "IdiomFunction" => Function,
        // `$param` — in SurrealQL a parameter is exactly what it looks like.
        "VariableName" => Parameter,
        "KeyName" | "ObjectKey" => Property,
        "RecordTbIdent" => Type,
        "RecordIdIdent" => EnumMember,
        "Operator" | "RangeOp" | "LookupLeft" | "LookupRight" | "LookupBoth" | "Any" | "At"
        | "Optional" | "Flatten" | "Pipe" => Operator,
        // A bare name is a variable; the same name reached through a path is
        // a field of whatever the path walked into.
        "Ident" => {
            if node
                .parent()
                .is_some_and(|parent| matches!(parent.kind(), "Path" | "Subscript" | "Lookup"))
            {
                Property
            } else {
                Variable
            }
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_source;
    use crate::source::SourceId;

    fn parse(text: &str) -> ParsedSource {
        parse_source(SourceId::new("t.surql"), text).expect("parses")
    }

    /// `(covered text, token type)` for each token, which is what actually
    /// matters: the ranges have to land on the right bytes.
    fn described(text: &str) -> Vec<std::string::String> {
        tokens(&parse(text))
            .into_iter()
            .map(|token| format!("{} {}", token.kind.lsp_name(), &text[token.range]))
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
    fn the_legend_index_is_the_variant_order() {
        // The index is a wire value: an LSP client is handed a legend and then
        // told "type 9", and the plugin maps the same number into TypeScript's
        // vocabulary. A silent renumber miscolours every token at once.
        assert_eq!(TokenKind::Keyword.index(), 0);
        assert_eq!(TokenKind::Parameter.index(), 9);
        assert_eq!(TokenKind::EnumMember.index(), 11);
    }
}
