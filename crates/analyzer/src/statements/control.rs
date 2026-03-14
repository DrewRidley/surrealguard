use tree_sitter::Node;

use crate::context::Context;
use crate::diagnostic::{Code, Diagnostic};
use crate::resolve::resolve_expr;
use crate::span::Span;
use crate::types::Kind;

pub fn analyze_if(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    resolve_expr(node, source, ctx, None)
}

pub fn analyze_for(node: &Node, source: &str, ctx: &mut Context) {
    resolve_expr(node, source, ctx, None);
}

pub fn analyze_let(node: &Node, source: &str, ctx: &mut Context) {
    resolve_expr(node, source, ctx, None);
}

pub fn analyze_return(node: &Node, source: &str, ctx: &mut Context) {
    resolve_expr(node, source, ctx, None);
}

pub fn analyze_throw(node: &Node, source: &str, ctx: &mut Context) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "keyword_throw" {
            resolve_expr(&child, source, ctx, None);
        }
    }
}

pub fn analyze_block(node: &Node, source: &str, ctx: &mut Context) {
    resolve_expr(node, source, ctx, None);
}

/// Validate that BREAK is used inside a loop.
///
/// If a `break_statement` reaches the top-level statement dispatcher it is
/// not nested inside a `for_statement` (whose handler resolves its own
/// children), so it must be outside a loop.
pub fn analyze_break(node: &Node, _source: &str, ctx: &mut Context) {
    let span = Span::from_node(node);
    ctx.emit(Diagnostic::error(
        span,
        Code::BreakOutsideLoop,
        "BREAK used outside of a loop".to_string(),
    ));
}

/// Validate that CONTINUE is used inside a loop.
///
/// Same reasoning as `analyze_break` — if the node reaches the dispatcher
/// it is not nested inside a loop body.
pub fn analyze_continue(node: &Node, _source: &str, ctx: &mut Context) {
    let span = Span::from_node(node);
    ctx.emit(Diagnostic::error(
        span,
        Code::ContinueOutsideLoop,
        "CONTINUE used outside of a loop".to_string(),
    ));
}

#[cfg(test)]
mod tests {
    use crate::parser;
    use crate::statements::analyze_all;

    #[test]
    fn break_outside_loop_emits_error() {
        let src = "BREAK;";
        let tree = parser::parse(src).unwrap();
        let mut ctx = crate::Context::new();
        analyze_all(&tree.root_node(), src, &mut ctx);
        let diags = &ctx.diagnostics;
        assert!(
            diags.iter().any(|d| d.code == crate::diagnostic::Code::BreakOutsideLoop),
            "Expected BreakOutsideLoop diagnostic, got: {:?}",
            diags
        );
    }

    #[test]
    fn continue_outside_loop_emits_error() {
        let src = "CONTINUE;";
        let tree = parser::parse(src).unwrap();
        let mut ctx = crate::Context::new();
        analyze_all(&tree.root_node(), src, &mut ctx);
        let diags = &ctx.diagnostics;
        assert!(
            diags.iter().any(|d| d.code == crate::diagnostic::Code::ContinueOutsideLoop),
            "Expected ContinueOutsideLoop diagnostic, got: {:?}",
            diags
        );
    }

    #[test]
    fn break_inside_for_loop_no_error() {
        let src = "FOR $x IN [1,2,3] { BREAK; };";
        let tree = parser::parse(src).unwrap();
        let mut ctx = crate::Context::new();
        analyze_all(&tree.root_node(), src, &mut ctx);
        let diags = &ctx.diagnostics;
        assert!(
            !diags.iter().any(|d| d.code == crate::diagnostic::Code::BreakOutsideLoop),
            "BREAK inside FOR should not produce BreakOutsideLoop, got: {:?}",
            diags
        );
    }

    #[test]
    fn continue_inside_for_loop_no_error() {
        let src = "FOR $x IN [1,2,3] { CONTINUE; };";
        let tree = parser::parse(src).unwrap();
        let mut ctx = crate::Context::new();
        analyze_all(&tree.root_node(), src, &mut ctx);
        let diags = &ctx.diagnostics;
        assert!(
            !diags.iter().any(|d| d.code == crate::diagnostic::Code::ContinueOutsideLoop),
            "CONTINUE inside FOR should not produce ContinueOutsideLoop, got: {:?}",
            diags
        );
    }
}
