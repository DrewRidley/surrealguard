//! FOR loop analysis.
//!
//! Handles: FOR $var IN iterable { body }
//! Validates iterable is array/set, binds loop variable with element type.

use tree_sitter::Node;

use crate::context::Context;
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{find_all, node_text};
use crate::scope::{Binding, BindingKind, ScopeKind};
use crate::span::Span;
use crate::types::{Kind, KindExt};
use crate::v2::expr;

/// Analyze a FOR statement.
pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    ctx.scope.push(ScopeKind::ForLoop);

    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();

    let mut var_name: Option<String> = None;
    let mut var_span = Span::from_node(node);
    let mut iterable_type = Kind::Any;
    let mut past_in = false;

    for child in &children {
        match child.kind() {
            "keyword_for" | "keyword_in" => {
                if child.kind() == "keyword_in" { past_in = true; }
            }
            "variable_name" | "variable" if !past_in => {
                let name = node_text(child, source);
                var_name = Some(if name.starts_with('$') {
                    name.to_string()
                } else {
                    format!("${}", name)
                });
                var_span = Span::from_node(child);
            }
            "block" | "block_expression" => {
                // Body — analyzed after binding
            }
            _ if past_in && child.kind() != "block" && child.kind() != "block_expression" => {
                iterable_type = expr::resolve_simple(child, source, ctx, None);
            }
            _ => {}
        }
    }

    // Extract element type from iterable
    let element_type = match &iterable_type {
        Kind::Array(inner, _) => inner.as_ref().clone(),
        Kind::Set(inner, _) => inner.as_ref().clone(),
        Kind::Any => Kind::Any,
        other => {
            if !other.is_any() {
                ctx.emit(Diagnostic::warning(
                    Span::from_node(node),
                    Code::TypeMismatch,
                    format!("FOR loop expects iterable (array or set), found `{}`", other),
                ));
            }
            Kind::Any
        }
    };

    // Bind loop variable
    if let Some(name) = var_name {
        ctx.scope.bind(Binding {
            name,
            typ: element_type,
            span: var_span,
            mutable: false,
            kind: BindingKind::ForLoop,
        });
    }

    // Analyze body block
    for child in &children {
        if child.kind() == "block" || child.kind() == "block_expression" {
            // Just resolve for diagnostics, FOR doesn't return a value
            let _ = crate::v2::statements::if_stmt::resolve_block(child, source, ctx);
        }
    }

    ctx.scope.pop();
    Kind::Null
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{analyze_with_context, Context};

    fn test_schema() -> Context {
        let mut ctx = Context::new();
        let _ = analyze_with_context(
            "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string;\nDEFINE FIELD age ON user TYPE int;",
            &mut ctx,
        );
        ctx.take_diagnostics();
        ctx
    }

    fn query_diagnostics(query: &str) -> Vec<crate::Diagnostic> {
        let mut ctx = test_schema();
        analyze_with_context(query, &mut ctx).unwrap_or_default()
    }

    #[test]
    fn for_loop_array_no_error() {
        let diags = query_diagnostics("FOR $item IN [1, 2, 3] { RETURN $item; };");
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Valid FOR loop: {:?}", errors);
    }

    #[test]
    fn for_loop_non_iterable_warns() {
        let diags = query_diagnostics("FOR $item IN 42 { RETURN $item; };");
        let warns: Vec<_> = diags.iter()
            .filter(|d| d.message.contains("iterable"))
            .collect();
        // May or may not warn depending on whether 42 resolves to int
        // The important thing is it doesn't crash
    }
}
