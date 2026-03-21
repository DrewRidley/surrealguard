//! DEFINE statement analysis.
//!
//! DEFINE TABLE, DEFINE FIELD, DEFINE FUNCTION, DEFINE INDEX, DEFINE EVENT, etc.
//! Schema extraction is handled by schema.rs — this module validates
//! the statements themselves (e.g., DEFAULT type mismatches, ASSERT conditions).

use tree_sitter::Node;

use crate::context::Context;
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{child_by_kind, node_text};
use crate::span::Span;
use crate::types::{Kind, KindExt};
use crate::v2::expr;

/// Analyze a DEFINE statement. Returns Kind::Null (definitions don't produce values).
pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();

    // Determine which kind of DEFINE
    for child in &children {
        match child.kind() {
            "define_table_statement" => analyze_define_table(child, source, ctx),
            "define_field_statement" => analyze_define_field(child, source, ctx),
            "define_function_statement" => analyze_define_function(child, source, ctx),
            "define_index_statement" => { /* validated in schema.rs */ }
            "define_event_statement" => { /* TODO */ }
            "define_param_statement" => analyze_define_param(child, source, ctx),
            _ => {}
        }
    }

    // If the node itself is a specific define statement (not wrapped)
    match node.kind() {
        "define_field_statement" => analyze_define_field(node, source, ctx),
        "define_function_statement" => analyze_define_function(node, source, ctx),
        "define_param_statement" => analyze_define_param(node, source, ctx),
        _ => {}
    }

    Kind::Null
}

fn analyze_define_table(_node: &Node, _source: &str, _ctx: &mut Context) {
    // Table validation (schema mode, permissions) handled by schema.rs
}

fn analyze_define_field(node: &Node, source: &str, ctx: &mut Context) {
    // Validate DEFAULT expression type matches field type
    let field_type = child_by_kind(node, "type_clause")
        .and_then(|tc| child_by_kind(&tc, "type"))
        .and_then(|t| crate::schema::parse_type(&t, source));

    if let Some(ref expected) = field_type {
        // Check DEFAULT clause type
        if let Some(default_clause) = child_by_kind(node, "default_clause") {
            let mut cursor = default_clause.walk();
            let children: Vec<_> = default_clause.named_children(&mut cursor).collect();
            for child in &children {
                if !child.kind().starts_with("keyword_") {
                    let actual = expr::resolve_simple(child, source, ctx, None);
                    if !actual.is_any() && !crate::types::is_assignable(expected, &actual) {
                        ctx.emit(Diagnostic::warning(
                            Span::from_node(child),
                            Code::DefaultTypeMismatch,
                            format!("expected `{}`, found `{}`", expected, actual),
                        ));
                    }
                }
            }
        }

        // Check ASSERT clause evaluates to bool
        if let Some(assert_clause) = child_by_kind(node, "assert_clause") {
            let mut cursor = assert_clause.walk();
            let children: Vec<_> = assert_clause.named_children(&mut cursor).collect();
            for child in &children {
                if !child.kind().starts_with("keyword_") {
                    let assert_type = expr::resolve_simple(child, source, ctx, None);
                    if assert_type != Kind::Bool && assert_type != Kind::Any {
                        ctx.emit(Diagnostic::warning(
                            Span::from_node(child),
                            Code::AssertNotBool,
                            format!("ASSERT should evaluate to `bool`, found `{}`", assert_type),
                        ));
                    }
                }
            }
        }
    }
}

fn analyze_define_function(node: &Node, source: &str, ctx: &mut Context) {
    // Validate return type matches body type
    let declared_return = child_by_kind(node, "returns_clause")
        .and_then(|rc| child_by_kind(&rc, "type"))
        .and_then(|t| crate::schema::parse_type(&t, source));

    if let Some(body) = child_by_kind(node, "block") {
        let body_type = crate::v2::statements::if_stmt::resolve_block(&body, source, ctx);
        if let Some(ref declared) = declared_return {
            if !declared.is_any() && !body_type.is_any()
                && !crate::types::is_assignable(declared, &body_type)
            {
                ctx.emit(Diagnostic::warning(
                    Span::from_node(&body),
                    Code::ReturnTypeMismatch,
                    format!("expected `{}`, found `{}`", declared, body_type),
                ));
            }
        }
    }
}

fn analyze_define_param(node: &Node, source: &str, ctx: &mut Context) {
    // Bind param in scope with inferred type
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();

    let mut name: Option<String> = None;
    let mut span = Span::from_node(node);

    for child in &children {
        if child.kind() == "variable_name" || child.kind() == "variable" {
            let n = node_text(child, source);
            name = Some(if n.starts_with('$') { n.to_string() } else { format!("${}", n) });
            span = Span::from_node(child);
        }
    }

    if let Some(var_name) = name {
        // Infer type from TYPE clause or VALUE expression
        let typ = child_by_kind(node, "type_clause")
            .and_then(|tc| child_by_kind(&tc, "type"))
            .and_then(|t| crate::schema::parse_type(&t, source))
            .or_else(|| {
                children.iter()
                    .find(|c| !c.kind().starts_with("keyword_") && c.kind() != "variable_name" && c.kind() != "variable")
                    .map(|v| expr::resolve_simple(v, source, ctx, None))
            })
            .unwrap_or(Kind::Any);

        ctx.scope.bind(crate::scope::Binding {
            name: var_name,
            typ,
            span,
            mutable: false,
            kind: crate::scope::BindingKind::DefineParam,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{analyze_with_context, Context};

    #[test]
    fn define_table_no_error() {
        let mut ctx = Context::new();
        let diags = analyze_with_context("DEFINE TABLE user SCHEMAFULL;", &mut ctx).unwrap_or_default();
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "DEFINE TABLE: {:?}", errors);
    }

    #[test]
    fn define_field_no_error() {
        let mut ctx = Context::new();
        let diags = analyze_with_context(
            "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string;",
            &mut ctx,
        ).unwrap_or_default();
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "DEFINE FIELD: {:?}", errors);
    }

    #[test]
    fn define_param_binds_type() {
        let mut ctx = Context::new();
        let _ = analyze_with_context("DEFINE PARAM $limit VALUE 10;", &mut ctx);
        ctx.take_diagnostics();
        let binding = ctx.scope.lookup("$limit");
        assert!(binding.is_some(), "$limit should be in scope");
    }
}
