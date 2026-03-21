//! REMOVE statement analysis.
//!
//! REMOVE TABLE/FIELD/INDEX/FUNCTION/EVENT/PARAM
//! Validates the target exists (in strict mode).

use tree_sitter::Node;

use crate::context::Context;
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{find_all, node_text};
use crate::span::Span;
use crate::types::Kind;

/// Analyze a REMOVE statement.
pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    if ctx.strict {
        // Check if the thing being removed exists
        let mut cursor = node.walk();
        let children: Vec<_> = node.named_children(&mut cursor).collect();

        let has_if_exists = children.iter().any(|c| c.kind() == "if_not_exists_clause");
        if has_if_exists { return Kind::Null; }

        let is_table = children.iter().any(|c| c.kind() == "keyword_table");
        let is_field = children.iter().any(|c| c.kind() == "keyword_field");

        for child in &children {
            if child.kind() == "identifier" {
                let name = node_text(child, source);
                if is_table && !ctx.has_table(name) {
                    ctx.emit(Diagnostic::error(
                        Span::from_node(child),
                        Code::TableNotFound,
                        format!("table `{}` is not defined", name),
                    ));
                }
            }
        }
    }
    Kind::Null
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{analyze_with_context, Context};

    #[test]
    fn remove_existing_no_error() {
        let mut ctx = Context::new();
        let _ = analyze_with_context("DEFINE TABLE user SCHEMAFULL;", &mut ctx);
        ctx.take_diagnostics();
        let diags = analyze_with_context("REMOVE TABLE user;", &mut ctx).unwrap_or_default();
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Remove existing: {:?}", errors);
    }

    #[test]
    fn remove_nonexistent_strict() {
        let mut ctx = Context::strict();
        let diags = analyze_with_context("REMOVE TABLE ghost;", &mut ctx).unwrap_or_default();
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::TableNotFound)
            .collect();
        assert!(!errors.is_empty(), "Strict should error: {:?}", diags);
    }
}
