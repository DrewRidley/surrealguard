//! CST → AST lowering.
//!
//! The single place that reads tree-sitter nodes and source text. Everything
//! downstream (analyzers) consumes [`crate::ast`] values.
//!
//! Constructs without a modeled lowering — and statements whose direct
//! syntax is broken — lower to explicit `Partial` values rather than being
//! silently dropped.

mod expr;
mod statement;

pub use expr::{lower_expr, lower_type_expr};
pub(crate) use statement::lower_statement;
pub use statement::lower_statements;

use crate::ast::{Expr, PartialNode, Script, Spanned, Statement};
use crate::parse::ParsedSource;
use crate::span::ByteRange;
use tree_sitter::Node;

/// Lowers the first CST node of `cst_kind` (depth-first) as an expression.
///
/// The node-walking entry point for tests and tools that target a specific
/// sub-expression (`FunctionCall`, `Block`, `BinaryExpression`, ...) without
/// reaching for tree-sitter themselves.
pub fn lower_first_expr(parsed: &ParsedSource, cst_kind: &str) -> Option<Spanned<Expr>> {
    let node = find_first(parsed.tree().root_node(), cst_kind)?;
    Some(lower_expr(node, parsed.text()))
}

/// Lowers the first CST node of `cst_kind` (depth-first) as a statement.
pub fn lower_first_statement(parsed: &ParsedSource, cst_kind: &str) -> Option<Spanned<Statement>> {
    let node = find_first(parsed.tree().root_node(), cst_kind)?;
    Some(lower_statement(node, parsed.text()))
}

fn find_first<'tree>(node: Node<'tree>, cst_kind: &str) -> Option<Node<'tree>> {
    if node.kind() == cst_kind {
        return Some(node);
    }
    let mut cursor = node.walk();
    let children: Vec<Node<'tree>> = node.children(&mut cursor).collect();
    children
        .into_iter()
        .find_map(|child| find_first(child, cst_kind))
}

/// Lowers a parsed source to a [`Script`], statements in source order.
pub fn lower(parsed: &ParsedSource) -> Script {
    let root = parsed.tree().root_node();
    let mut statements = Vec::new();

    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if !child.is_named() || matches!(child.kind(), "Comment" | "BlockComment") {
            continue;
        }
        statements.push(statement::lower_statement(child, parsed.text()));
    }

    Script { statements }
}

pub(crate) fn node_range(node: Node<'_>) -> ByteRange {
    let start = u32::try_from(node.start_byte()).unwrap_or(u32::MAX);
    let end = u32::try_from(node.end_byte()).unwrap_or(u32::MAX);
    ByteRange::new(start, end).expect("tree-sitter nodes have ordered byte ranges")
}

pub(crate) fn partial(node: Node<'_>) -> PartialNode {
    let cst_kind = if node.is_missing() {
        format!("MISSING {}", node.kind())
    } else {
        node.kind().to_string()
    };
    PartialNode {
        span: node_range(node),
        cst_kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Statement;
    use crate::parse::parse_source;
    use crate::source::SourceId;

    #[test]
    fn script_lowering_preserves_statement_order() {
        let parsed = parse_source(
            SourceId::new("lower:script"),
            "DEFINE TABLE person;\nSELECT * FROM person;\nRETURN 1;",
        )
        .expect("parses");

        let script = lower(&parsed);

        assert!(matches!(&script.statements[0].node, Statement::Define(_)));
        assert!(matches!(&script.statements[1].node, Statement::Select(_)));
        assert!(matches!(&script.statements[2].node, Statement::Return(_)));
        // Source order is positional: each statement's span starts after the
        // previous one ends.
        let spans: Vec<_> = script.statements.iter().map(|s| s.span).collect();
        assert!(spans.windows(2).all(|w| w[0].end() <= w[1].start()));
    }
}
