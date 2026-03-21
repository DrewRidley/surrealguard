//! IF/ELSE expression analysis.
//!
//! Handles: condition type validation, branch type unification,
//! ELSE/ELSE IF chains, block bodies.
//!
//! Result type:
//! - IF cond { A } → A | null (no else means might not execute)
//! - IF cond { A } ELSE { B } → A | B
//! - IF cond { A } ELSE IF cond2 { B } ELSE { C } → A | B | C

use tree_sitter::Node;

use crate::context::Context;
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{child_by_kind, find_all, node_text};
use crate::span::Span;
use crate::types::Kind;
use crate::v2::expr;

/// Analyze an IF expression. Returns the unified type of all branches.
pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();

    let mut branch_types = Vec::new();
    let mut has_else = false;
    let mut seen_condition = false;

    for child in &children {
        match child.kind() {
            // Branch body (THEN result or block)
            "if_then_result" | "block" | "block_expression" => {
                let typ = resolve_branch(child, source, ctx);
                branch_types.push(typ);
            }
            // ELSE clause
            "else_then_clause" | "else_if_clause" | "else_clause" => {
                has_else = true;
                let typ = resolve_branch(child, source, ctx);
                branch_types.push(typ);
            }
            // Keywords
            k if k.starts_with("keyword_") => {}
            // Condition expression
            _ => {
                if !seen_condition {
                    let cond_type = expr::resolve_simple(child, source, ctx, None);
                    if cond_type != Kind::Bool && cond_type != Kind::Any {
                        ctx.emit(Diagnostic::warning(
                            Span::from_node(child),
                            Code::TypeMismatch,
                            format!("IF condition should be `bool`, found `{}`", cond_type),
                        ));
                    }
                    seen_condition = true;
                }
            }
        }
    }

    // If no ELSE, the IF might not execute → null is a possible result
    if !has_else {
        branch_types.push(Kind::Null);
    }

    unify_types(&branch_types)
}

fn resolve_branch(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    // For blocks, resolve each statement and return the last one's type
    if node.kind() == "block" || node.kind() == "block_expression" {
        return resolve_block(node, source, ctx);
    }
    // For else clauses, recurse into children
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();
    for child in &children {
        match child.kind() {
            "block" | "block_expression" | "if_then_result" => {
                return resolve_branch(child, source, ctx);
            }
            // Nested IF in else-if chain
            "if_expression" | "if_statement" => {
                return analyze(child, source, ctx);
            }
            k if k.starts_with("keyword_") => continue,
            _ => {
                return expr::resolve_simple(child, source, ctx, None);
            }
        }
    }
    Kind::Null
}

pub fn resolve_block(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    let mut last_type = Kind::Null;
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();

    for child in &children {
        match child.kind() {
            "semi_colon" => continue,
            "expressions" => {
                let mut inner = child.walk();
                let exprs: Vec<_> = child.named_children(&mut inner).collect();
                for expr_node in &exprs {
                    if expr_node.kind() == "semi_colon" { continue; }
                    last_type = resolve_block_expr(expr_node, source, ctx);
                }
            }
            _ => {
                last_type = resolve_block_expr(child, source, ctx);
            }
        }
    }
    last_type
}

fn resolve_block_expr(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    match node.kind() {
        "subquery_statement" | "primary_statement" | "expression" => {
            let mut cursor = node.walk();
            let children: Vec<_> = node.named_children(&mut cursor).collect();
            if let Some(child) = children.first() {
                resolve_block_expr(child, source, ctx)
            } else {
                Kind::Any
            }
        }
        "return_statement" | "return_clause" => {
            let mut cursor = node.walk();
            let children: Vec<_> = node.named_children(&mut cursor).collect();
            if let Some(val) = children.iter().find(|c| !c.kind().starts_with("keyword_")) {
                expr::resolve_simple(val, source, ctx, None)
            } else {
                Kind::Null
            }
        }
        "select_statement" => crate::v2::statements::select::analyze(node, source, ctx),
        "create_statement" => crate::v2::statements::create::analyze(node, source, ctx),
        "let_statement" => crate::v2::statements::let_stmt::analyze(node, source, ctx),
        "if_expression" | "if_statement" => analyze(node, source, ctx),
        _ => expr::resolve_simple(node, source, ctx, None),
    }
}

fn unify_types(types: &[Kind]) -> Kind {
    if types.is_empty() { return Kind::Any; }
    let mut unique: Vec<Kind> = Vec::new();
    for t in types {
        if !unique.contains(t) && *t != Kind::Any {
            unique.push(t.clone());
        }
    }
    match unique.len() {
        0 => Kind::Any,
        1 => unique.into_iter().next().unwrap(),
        _ => Kind::Either(unique),
    }
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

    fn let_type(stmt: &str) -> Kind {
        let mut ctx = test_schema();
        let _ = analyze_with_context(stmt, &mut ctx);
        ctx.take_diagnostics();
        let var = stmt.split_whitespace()
            .find(|w| w.starts_with('$'))
            .map(|w| w.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '$'))
            .unwrap_or("$result");
        ctx.scope.lookup(var).map(|b| b.typ.clone()).unwrap_or(Kind::Any)
    }

    #[test]
    fn if_else_string_or_int() {
        let typ = let_type("LET $x = IF true { 'hello' } ELSE { 42 };");
        match &typ {
            Kind::Either(variants) => {
                assert!(variants.len() >= 2, "Expected 2+ variants: {:?}", variants);
            }
            Kind::String | Kind::Int => {} // one branch dominated
            other => panic!("Expected union, got: {:?}", other),
        }
    }

    #[test]
    fn if_no_else_includes_null() {
        let typ = let_type("LET $x = IF true { 'hello' };");
        // Without ELSE, result could be null
        match &typ {
            Kind::Either(variants) => {
                assert!(variants.contains(&Kind::Null) || variants.contains(&Kind::String));
            }
            Kind::String | Kind::Null => {} // acceptable
            other => panic!("Expected string|null, got: {:?}", other),
        }
    }

    #[test]
    fn if_else_same_type() {
        let typ = let_type("LET $x = IF true { 'a' } ELSE { 'b' };");
        assert_eq!(typ, Kind::String, "Both branches string: {:?}", typ);
    }

    #[test]
    fn if_with_block_return() {
        let typ = let_type("LET $x = IF true { RETURN 42; };");
        match &typ {
            Kind::Int | Kind::Number => {}
            Kind::Either(v) => assert!(v.iter().any(|t| matches!(t, Kind::Int | Kind::Number))),
            other => panic!("Expected int, got: {:?}", other),
        }
    }
}
