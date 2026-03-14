/// Tree-sitter parsing utilities for SurrealQL.
///
/// Provides helpers to parse SurrealQL source text and navigate
/// the concrete syntax tree (CST).
use tree_sitter::{Node, Parser, Tree};

use crate::span::Span;

/// Parse SurrealQL source text into a tree-sitter tree.
pub fn parse(source: &str) -> Result<Tree, ParseError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_surrealql::LANGUAGE.into())
        .map_err(|_| ParseError::LanguageError)?;

    parser.parse(source, None).ok_or(ParseError::ParseFailed)
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("Failed to load SurrealQL tree-sitter language")]
    LanguageError,
    #[error("Tree-sitter parse returned None")]
    ParseFailed,
}

/// Get the text of a tree-sitter node from the source.
pub fn node_text<'a>(node: &Node, source: &'a str) -> &'a str {
    &source[node.start_byte()..node.end_byte()]
}

/// Find a child node by its kind (field name).
pub fn child_by_kind<'a>(node: &Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let result = node.children(&mut cursor)
        .find(|child| child.kind() == kind);
    result
}

/// Find all children of a specific kind.
pub fn children_by_kind<'a>(node: &'a Node<'a>, kind: &str) -> Vec<Node<'a>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| child.kind() == kind)
        .collect()
}

/// Iterator over named children of a node.
pub fn named_children<'a>(node: &'a Node<'a>) -> impl Iterator<Item = Node<'a>> + 'a {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect::<Vec<_>>().into_iter()
}

/// Get the span of a tree-sitter node.
pub fn node_span(node: &Node) -> Span {
    Span::from_node(node)
}

/// Check if a node has errors (useful for partial parse recovery).
pub fn has_error(node: &Node) -> bool {
    node.has_error()
}

/// Walk all descendants of a node matching a kind.
pub fn find_all<'a>(node: &Node<'a>, kind: &str) -> Vec<Node<'a>> {
    let mut results = Vec::new();
    let mut cursor = node.walk();
    walk_recursive(node, &mut cursor, kind, &mut results);
    results
}

fn walk_recursive<'a>(
    node: &Node<'a>,
    cursor: &mut tree_sitter::TreeCursor<'a>,
    kind: &str,
    results: &mut Vec<Node<'a>>,
) {
    if node.kind() == kind {
        results.push(*node);
    }
    let mut child_cursor = node.walk();
    for child in node.children(&mut child_cursor) {
        walk_recursive(&child, cursor, kind, results);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_select() {
        let tree = parse("SELECT * FROM person;").unwrap();
        let root = tree.root_node();
        assert_eq!(root.kind(), "source_file");
        assert!(!root.has_error());
    }

    #[test]
    fn parse_define_table() {
        let tree = parse("DEFINE TABLE user SCHEMAFULL;").unwrap();
        let root = tree.root_node();
        assert!(!root.has_error());

        let stmts = find_all(&root, "define_table_statement");
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn parse_with_error_recovery() {
        let tree = parse("SELECT * FROM ;").unwrap();
        let root = tree.root_node();
        // Tree-sitter recovers from errors — we still get a tree
        assert!(root.has_error());
    }

    #[test]
    fn parse_with_index_clause() {
        let source = "SELECT * FROM user WITH INDEX idx_name;";
        let tree = parse(source).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error(), "WITH INDEX should parse without errors");

        let with_clauses = find_all(&root, "with_clause");
        assert_eq!(with_clauses.len(), 1, "Should find one with_clause node");

        let wc = &with_clauses[0];
        assert!(child_by_kind(wc, "keyword_with").is_some());
        assert!(child_by_kind(wc, "keyword_index").is_some());

        let index_names: Vec<_> = find_all(wc, "identifier")
            .iter()
            .map(|n| node_text(n, source))
            .collect();
        assert_eq!(index_names, vec!["idx_name"]);
    }

    #[test]
    fn node_text_works() {
        let source = "SELECT name FROM person;";
        let tree = parse(source).unwrap();
        let root = tree.root_node();

        let identifiers = find_all(&root, "identifier");
        assert!(identifiers.len() >= 2);
        let texts: Vec<_> = identifiers.iter().map(|n| node_text(n, source)).collect();
        assert!(texts.contains(&"name"));
        assert!(texts.contains(&"person"));
    }
}
