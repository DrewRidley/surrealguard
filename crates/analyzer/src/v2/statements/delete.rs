//! DELETE statement analysis.
//!
//! Handles: target table validation, WHERE clause, RETURN clause, ONLY modifier.
//!
//! Result type:
//! - DELETE table → array<table_type>
//! - DELETE ONLY table → table_type
//! - DELETE table RETURN NONE → null

use tree_sitter::Node;

use crate::context::Context;
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{child_by_kind, find_all, node_text};
use crate::span::Span;
use crate::types::{Kind, KindExt};
use crate::v2::expr;

/// Analyze a DELETE statement. Returns the result type.
pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    let table = resolve_target_table(node, source);
    let is_only = find_all(node, "keyword_only").len() > 0;

    if let Some(ref tbl) = table {
        if ctx.strict && !ctx.has_table(tbl) {
            ctx.emit(Diagnostic::error(
                find_target_span(node, source),
                Code::TableNotFound,
                format!("table `{}` is not defined", tbl),
            ));
        }
    }

    // Validate WHERE
    if let Some(where_clause) = child_by_kind(node, "where_clause") {
        let mut cursor = where_clause.walk();
        let children: Vec<_> = where_clause.named_children(&mut cursor).collect();
        for child in &children {
            if child.kind() != "keyword_where" {
                let typ = expr::resolve_simple(child, source, ctx, table.as_deref());
                if typ != Kind::Bool && typ != Kind::Any {
                    ctx.emit(Diagnostic::warning(
                        Span::from_node(child),
                        Code::TypeMismatch,
                        format!("WHERE condition should be `bool`, found `{}`", typ),
                    ));
                }
            }
        }
    }

    // Validate RETURN clause field references
    validate_return_fields(node, source, ctx, table.as_deref());

    let table_type = table.as_deref()
        .and_then(|tbl| ctx.build_table_type(tbl))
        .unwrap_or(Kind::Any);

    let result = process_return_clause(node, &table_type);

    if is_only { result } else { Kind::Array(Box::new(result), None) }
}

fn resolve_target_table(node: &Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();
    for child in &children {
        if child.kind().ends_with("_clause") { break; }
        if child.kind() == "keyword_delete" || child.kind() == "keyword_only" { continue; }
        if child.kind() == "identifier" {
            return Some(node_text(child, source).to_string());
        }
        for ident in find_all(child, "identifier") {
            return Some(node_text(&ident, source).to_string());
        }
    }
    None
}

fn find_target_span(node: &Node, source: &str) -> Span {
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();
    for child in &children {
        if child.kind().ends_with("_clause") { break; }
        if child.kind() == "identifier" { return Span::from_node(child); }
    }
    Span::from_node(node)
}

fn validate_return_fields(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    for rc in find_all(node, "return_clause") {
        for ident in find_all(&rc, "identifier") {
            let name = node_text(&ident, source);
            if let Some(tbl) = table {
                if let Some(td) = ctx.get_table(tbl) {
                    if td.schema_mode == crate::context::SchemaMode::Schemafull
                        && ctx.get_field(tbl, name).is_none()
                        && name != "id"
                    {
                        ctx.emit(Diagnostic::warning(
                            Span::from_node(&ident),
                            Code::FieldNotFound,
                            format!("field `{}` in RETURN clause not defined on table `{}`", name, tbl),
                        ));
                    }
                }
            }
        }
    }
}

fn process_return_clause(node: &Node, table_type: &Kind) -> Kind {
    for rc in find_all(node, "return_clause") {
        let mut cursor = rc.walk();
        let children: Vec<_> = rc.named_children(&mut cursor).collect();
        for child in &children {
            match child.kind() {
                "keyword_none" => return Kind::Null,
                "keyword_before" => return table_type.clone(),
                "keyword_after" => return table_type.clone(),
                "keyword_diff" => return Kind::Any,
                _ => {}
            }
        }
    }
    table_type.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{analyze_with_context, Context};

    fn test_schema() -> Context {
        let mut ctx = Context::new();
        let _ = analyze_with_context(
            r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
                DEFINE FIELD age ON user TYPE int;
                DEFINE FIELD active ON user TYPE bool;

            DEFINE TABLE flex SCHEMALESS;
            "#,
            &mut ctx,
        );
        ctx.take_diagnostics();
        ctx
    }

    fn query_type(query: &str) -> Kind {
        let mut ctx = test_schema();
        let full = format!("LET $result = {};", query);
        let _ = analyze_with_context(&full, &mut ctx);
        ctx.take_diagnostics();
        ctx.scope.lookup("$result")
            .map(|b| b.typ.clone())
            .unwrap_or(Kind::Any)
    }

    fn query_diagnostics(query: &str) -> Vec<crate::Diagnostic> {
        let mut ctx = test_schema();
        analyze_with_context(query, &mut ctx).unwrap_or_default()
    }

    // ── Result type ──────────────────────────────────────────

    #[test]
    fn returns_array() {
        let typ = query_type("DELETE user");
        assert!(matches!(&typ, Kind::Array(_, _)), "Expected array, got: {:?}", typ);
    }

    #[test]
    fn return_none_gives_null() {
        let typ = query_type("DELETE user RETURN NONE");
        // RETURN NONE produces null — may or may not be wrapped in array
        let is_null = match &typ {
            Kind::Null => true,
            Kind::Array(inner, _) => matches!(inner.as_ref(), Kind::Null),
            _ => false,
        };
        assert!(is_null, "Expected null, got: {:?}", typ);
    }

    // ── WHERE clause ─────────────────────────────────────────

    #[test]
    fn where_valid_no_error() {
        let diags = query_diagnostics("DELETE user WHERE active = false");
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Valid WHERE: {:?}", errors);
    }

    // ── RETURN clause field validation ───────────────────────

    #[test]
    fn return_valid_field() {
        let diags = query_diagnostics("DELETE user RETURN name, age");
        let warns: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::FieldNotFound)
            .collect();
        assert!(warns.is_empty(), "Valid fields: {:?}", warns);
    }

    #[test]
    fn return_undefined_field() {
        let diags = query_diagnostics("DELETE user RETURN ghost");
        let warns: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::FieldNotFound)
            .collect();
        assert!(!warns.is_empty(), "Should warn on 'ghost': {:?}", diags);
    }

    // ── Strict mode ──────────────────────────────────────────

    #[test]
    fn strict_undefined_table() {
        let mut ctx = Context::strict();
        let diags = analyze_with_context("DELETE nonexistent", &mut ctx).unwrap_or_default();
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::TableNotFound)
            .collect();
        assert!(!errors.is_empty(), "Strict should error: {:?}", diags);
    }
}
