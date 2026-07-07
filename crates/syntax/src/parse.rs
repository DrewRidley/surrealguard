//! Parsing entry points and tree-sitter facade types.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Parser, Tree};

use crate::source::SourceId;
use crate::span::{ByteRange, SourceSpan};

#[derive(Debug)]
/// A source file plus its parse tree; the input to lowering and to the
/// syntax-diagnostic pass.
pub struct ParsedSource {
    source_id: SourceId,
    text: Arc<str>,
    tree: Tree,
    syntax_diagnostics: Vec<SyntaxDiagnostic>,
}

impl ParsedSource {
    pub fn source_id(&self) -> &SourceId {
        &self.source_id
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    pub fn root_kind(&self) -> &str {
        self.tree.root_node().kind()
    }

    pub fn has_error(&self) -> bool {
        self.tree.root_node().has_error()
    }

    pub fn syntax_diagnostics(&self) -> &[SyntaxDiagnostic] {
        &self.syntax_diagnostics
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// A parse-level problem (an `ERROR` or `MISSING` region) with its span.
pub struct SyntaxDiagnostic {
    kind: SyntaxDiagnosticKind,
    span: SourceSpan,
    message: String,
}

impl SyntaxDiagnostic {
    pub fn new(kind: SyntaxDiagnosticKind, span: SourceSpan, message: impl Into<String>) -> Self {
        Self {
            kind,
            span,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> SyntaxDiagnosticKind {
        self.kind
    }

    pub fn span(&self) -> &SourceSpan {
        &self.span
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
/// Whether the parser recovered by skipping input or by inserting a
/// missing token.
pub enum SyntaxDiagnosticKind {
    ErrorNode,
    MissingNode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Failure to produce a tree at all (as opposed to a recoverable
/// [`SyntaxDiagnostic`]).
pub enum ParseError {
    LanguageError,
    ParseFailed,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::LanguageError => {
                f.write_str("failed to load SurrealQL tree-sitter language")
            }
            ParseError::ParseFailed => f.write_str("tree-sitter parse returned None"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parses `text` into a [`ParsedSource`], collecting recoverable syntax
/// diagnostics rather than failing on imperfect input.
pub fn parse_source(
    source_id: SourceId,
    text: impl Into<Arc<str>>,
) -> Result<ParsedSource, ParseError> {
    let text = text.into();
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_surrealql::LANGUAGE.into())
        .map_err(|_| ParseError::LanguageError)?;

    let tree = parser
        .parse(text.as_ref(), None)
        .ok_or(ParseError::ParseFailed)?;
    let mut syntax_diagnostics = Vec::new();
    collect_syntax_diagnostics(tree.root_node(), &source_id, &mut syntax_diagnostics);

    Ok(ParsedSource {
        source_id,
        text,
        tree,
        syntax_diagnostics,
    })
}

fn collect_syntax_diagnostics(
    node: Node<'_>,
    source_id: &SourceId,
    diagnostics: &mut Vec<SyntaxDiagnostic>,
) {
    if node.is_error() {
        diagnostics.push(SyntaxDiagnostic::new(
            SyntaxDiagnosticKind::ErrorNode,
            node_source_span(node, source_id.clone()),
            "SurrealQL syntax error",
        ));
    }

    if node.is_missing() {
        diagnostics.push(SyntaxDiagnostic::new(
            SyntaxDiagnosticKind::MissingNode,
            node_source_span(node, source_id.clone()),
            format!("missing SurrealQL syntax node `{}`", node.kind()),
        ));
    }

    let diagnostics_before_children = diagnostics.len();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_syntax_diagnostics(child, source_id, diagnostics);
    }

    if node.has_error() && diagnostics.len() == diagnostics_before_children {
        diagnostics.push(SyntaxDiagnostic::new(
            SyntaxDiagnosticKind::ErrorNode,
            node_source_span(node, source_id.clone()),
            "SurrealQL syntax error",
        ));
    }
}

fn node_source_span(node: Node<'_>, source_id: SourceId) -> SourceSpan {
    let start = node.start_byte().min(u32::MAX as usize) as u32;
    let end = node.end_byte().min(u32::MAX as usize) as u32;
    let range = ByteRange::new(start, end).expect("tree-sitter node byte ranges are ordered");
    SourceSpan::new(source_id, range)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceId;

    #[test]
    fn parse_source_keeps_text_source_and_tree() {
        let parsed = parse_source(SourceId::new("query:001"), "SELECT * FROM person;")
            .expect("valid query should parse");

        assert_eq!(parsed.source_id().as_str(), "query:001");
        assert_eq!(parsed.text(), "SELECT * FROM person;");
        assert_eq!(parsed.root_kind(), "SurrealQL");
        assert!(!parsed.has_error());
        assert!(parsed.syntax_diagnostics().is_empty());
    }

    #[test]
    fn parse_source_collects_error_nodes_as_diagnostics() {
        let parsed = parse_source(SourceId::new("query:bad"), "SELECT * FROM ;")
            .expect("tree-sitter should still return a partial tree");

        assert!(parsed.has_error());
        assert!(!parsed.syntax_diagnostics().is_empty());
        assert!(parsed
            .syntax_diagnostics()
            .iter()
            .all(|diagnostic| diagnostic.span().source().as_str() == "query:bad"));
    }
}
