//! CREATE statement analysis.
//!
//! Handles: target table validation, SET clause field/type checking,
//! CONTENT clause validation, missing required fields, RETURN clause,
//! ONLY modifier, duplicate field assignments.
//!
//! Result type:
//! - CREATE table SET ... → array<table_type>
//! - CREATE ONLY table SET ... → table_type (no array)

use std::collections::HashSet;
use tree_sitter::Node;

use crate::context::{Context, SchemaMode};
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{child_by_kind, find_all, node_text};
use crate::span::Span;
use crate::types::{Kind, KindExt};
use crate::v2::expr;

/// Analyze a CREATE statement. Returns the result type.
pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    let table = resolve_target_table(node, source);
    let is_only = find_all(node, "keyword_only").len() > 0;

    // Validate table exists (in strict mode)
    if let Some(ref tbl) = table {
        if ctx.strict && !ctx.has_table(tbl) {
            ctx.emit(Diagnostic::error(
                find_target_span(node),
                Code::TableNotFound,
                format!("table `{}` is not defined", tbl),
            ));
        }
    }

    // Process SET clause
    let provided_fields = process_set_clause(node, source, ctx, table.as_deref());

    // Process CONTENT clause
    let content_fields = process_content_clause(node, source, ctx, table.as_deref());

    // Check for missing required fields
    let all_provided: HashSet<String> = provided_fields
        .iter()
        .chain(content_fields.iter())
        .cloned()
        .collect();
    check_missing_required(node, ctx, table.as_deref(), &all_provided);

    // Build result type
    let table_type = table
        .as_deref()
        .and_then(|tbl| ctx.build_table_type(tbl))
        .unwrap_or(Kind::Any);

    // Process RETURN clause if present
    let result = process_return_clause(node, &table_type);

    if is_only {
        result
    } else {
        Kind::Array(Box::new(result), None)
    }
}

// ── Target table ─────────────────────────────────────────────

fn resolve_target_table(node: &Node, source: &str) -> Option<String> {
    let target = child_by_kind(node, "create_target")?;
    let idents = find_all(&target, "identifier");
    if let Some(ident) = idents.first() {
        let name = node_text(ident, source);
        let table = name.split(':').next().unwrap_or(name);
        if !table.is_empty() {
            return Some(table.to_string());
        }
    }
    let rids = find_all(&target, "record_id");
    if let Some(rid) = rids.first() {
        let text = node_text(rid, source);
        if let Some(table) = text.split(':').next() {
            return Some(table.to_string());
        }
    }
    None
}

fn find_target_span(node: &Node) -> Span {
    child_by_kind(node, "create_target")
        .and_then(|t| find_all(&t, "identifier").first().map(|n| Span::from_node(n)))
        .unwrap_or_else(|| Span::from_node(node))
}

// ── SET clause ───────────────────────────────────────────────

fn process_set_clause(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
) -> Vec<String> {
    let mut provided = Vec::new();
    let mut seen = HashSet::new();

    for assignment in find_all(node, "field_assignment") {
        let idents = find_all(&assignment, "identifier");
        let Some(field_ident) = idents.first() else { continue };
        let field_name = node_text(field_ident, source);

        if !seen.insert(field_name.to_string()) {
            ctx.emit(Diagnostic::warning(
                Span::from_node(field_ident),
                Code::DuplicateFieldAssignment,
                format!("field `{}` is assigned multiple times", field_name),
            ));
        }

        provided.push(field_name.to_string());

        if let Some(tbl) = table {
            validate_field(field_ident, &assignment, field_name, tbl, source, ctx);
        }
    }

    provided
}

fn validate_field(
    field_ident: &Node,
    assignment: &Node,
    field_name: &str,
    table: &str,
    source: &str,
    ctx: &mut Context,
) {
    let is_schemafull = ctx.get_table(table)
        .map(|t| t.schema_mode == SchemaMode::Schemafull)
        .unwrap_or(false);

    let field_def = ctx.get_field(table, field_name);

    if field_def.is_none() && is_schemafull {
        ctx.emit(Diagnostic::warning(
            Span::from_node(field_ident),
            Code::UndefinedFieldOnSchemafull,
            format!("field `{}` is not defined on table `{}`", field_name, table),
        ));
        return;
    }

    // Clone field info to avoid borrow conflict with ctx
    let field_info = field_def.map(|f| (f.readonly, f.is_computed, f.typ.clone()));
    if let Some((readonly, computed, typ)) = field_info {
        if readonly {
            ctx.emit(Diagnostic::error(
                Span::from_node(field_ident),
                Code::ReadonlyAssignment,
                format!("cannot assign to readonly field `{}`", field_name),
            ));
        }
        if computed {
            ctx.emit(Diagnostic::error(
                Span::from_node(field_ident),
                Code::ComputedFieldAssignment,
                format!("cannot assign to computed field `{}`", field_name),
            ));
        }
        if let Some(ref expected) = typ {
            if let Some(val) = find_assignment_value(assignment) {
                let actual = expr::resolve_simple(&val, source, ctx, Some(table));
                if !actual.is_any() && !crate::types::is_assignable(expected, &actual) {
                    ctx.emit(Diagnostic::error(
                        Span::from_node(&val),
                        Code::IncompatibleAssignment,
                        format!("expected `{}`, found `{}`", expected, actual),
                    ));
                }
            }
        }
    }
}

fn find_assignment_value<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let children: Vec<Node<'a>> = node.named_children(&mut cursor).collect();
    children.into_iter().rev().find(|c| {
        c.kind() != "identifier" && c.kind() != "operator"
    })
}

// ── CONTENT clause ───────────────────────────────────────────

fn process_content_clause(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
) -> Vec<String> {
    let mut provided = Vec::new();
    let Some(content) = child_by_kind(node, "content_clause") else {
        return provided;
    };

    for obj in find_all(&content, "object") {
        for prop in find_all(&obj, "object_property") {
            let keys = find_all(&prop, "object_key");
            let idents_fallback = find_all(&prop, "identifier");
            let key = keys.first().or_else(|| idents_fallback.first()).copied();
            if let Some(key_node) = key {
                let key_name = node_text(&key_node, source);
                provided.push(key_name.to_string());

                if let Some(tbl) = table {
                    // Clone expected type to avoid borrow conflict
                    let expected_type = ctx.get_field(tbl, key_name)
                        .and_then(|f| f.typ.clone());
                    if let Some(ref expected) = expected_type {
                        let mut cursor = prop.walk();
                        let children: Vec<_> = prop.named_children(&mut cursor).collect();
                        if let Some(val) = children.last() {
                            if val.kind() != "object_key" {
                                let actual = expr::resolve_simple(val, source, ctx, Some(tbl));
                                if !actual.is_any() && !crate::types::is_assignable(expected, &actual) {
                                    ctx.emit(Diagnostic::error(
                                        Span::from_node(val),
                                        Code::IncompatibleAssignment,
                                        format!("expected `{}`, found `{}`", expected, actual),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    provided
}

// ── Missing required fields ──────────────────────────────────

fn check_missing_required(
    node: &Node,
    ctx: &mut Context,
    table: Option<&str>,
    provided: &HashSet<String>,
) {
    let Some(tbl) = table else { return };
    let required = ctx.required_fields(tbl);
    let target_span = find_target_span(node);

    for field_name in &required {
        if !provided.contains(field_name) {
            let mut diag = Diagnostic::warning(
                target_span,
                Code::MissingRequiredField,
                format!("missing required field `{}`\non table `{}`", field_name, tbl),
            );
            if let Some(field_def) = ctx.get_field(tbl, field_name) {
                let typ_str = field_def.typ.as_ref()
                    .map(|t| format!("{}", t))
                    .unwrap_or_else(|| "any".to_string());
                diag = diag.with_related(
                    field_def.span,
                    format!("`{}` defined as `{}` here", field_name, typ_str),
                );
            }
            ctx.emit(diag);
        }
    }
}

// ── RETURN clause ────────────────────────────────────────────

fn process_return_clause(node: &Node, table_type: &Kind) -> Kind {
    let return_clauses = find_all(node, "return_clause");
    if let Some(rc) = return_clauses.first() {
        let mut cursor = rc.walk();
        let children: Vec<_> = rc.named_children(&mut cursor).collect();
        for child in &children {
            match child.kind() {
                "keyword_none" => return Kind::Null,
                "keyword_before" => return Kind::Null,
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
                DEFINE FIELD email ON user TYPE string;
                DEFINE FIELD active ON user TYPE bool;
                DEFINE FIELD tags ON user TYPE array<string>;

            DEFINE TABLE flex SCHEMALESS;
                DEFINE FIELD name ON flex TYPE string;
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
    fn returns_array_of_table_type() {
        let typ = query_type("CREATE user SET name = 'Alice', age = 30, email = 'a@b.com', active = true");
        assert!(matches!(&typ, Kind::Array(_, _)), "Expected array, got: {:?}", typ);
    }

    // ── Compatible types ─────────────────────────────────────

    #[test]
    fn set_compatible_types_no_error() {
        let diags = query_diagnostics(
            "CREATE user SET name = 'Alice', age = 30, email = 'a@b.com', active = true"
        );
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Compatible types should not error: {:?}", errors);
    }

    // ── Type mismatch ────────────────────────────────────────

    #[test]
    fn set_type_mismatch() {
        let diags = query_diagnostics(
            "CREATE user SET name = 'Alice', age = 'twenty', email = 'a@b.com', active = true"
        );
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::IncompatibleAssignment)
            .collect();
        assert!(!errors.is_empty(), "Expected type mismatch: {:?}", diags);
    }

    #[test]
    fn content_type_mismatch() {
        let diags = query_diagnostics(
            "CREATE user CONTENT { name: 'Alice', age: 'not_int', email: 'a@b.com', active: true }"
        );
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::IncompatibleAssignment)
            .collect();
        assert!(!errors.is_empty(), "Expected CONTENT type mismatch: {:?}", diags);
    }

    // ── Undefined field ──────────────────────────────────────

    #[test]
    fn undefined_field_schemafull() {
        let diags = query_diagnostics(
            "CREATE user SET name = 'Alice', age = 30, email = 'a@b.com', active = true, ghost = 'boo'"
        );
        let warns: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::UndefinedFieldOnSchemafull)
            .collect();
        assert!(!warns.is_empty(), "Should warn on undefined 'ghost': {:?}", diags);
    }

    #[test]
    fn undefined_field_schemaless_ok() {
        let diags = query_diagnostics("CREATE flex SET name = 'test', anything = 'goes'");
        let warns: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::UndefinedFieldOnSchemafull)
            .collect();
        assert!(warns.is_empty(), "Schemaless allows any field: {:?}", warns);
    }

    // ── Missing required ─────────────────────────────────────

    #[test]
    fn missing_required_field() {
        let diags = query_diagnostics("CREATE user SET name = 'Alice'");
        let warns: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::MissingRequiredField)
            .collect();
        assert!(!warns.is_empty(), "Should warn on missing fields: {:?}", diags);
    }

    #[test]
    fn all_required_no_warning() {
        let diags = query_diagnostics(
            "CREATE user SET name = 'Alice', age = 30, email = 'a@b.com', active = true, tags = ['a']"
        );
        let warns: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::MissingRequiredField)
            .collect();
        assert!(warns.is_empty(), "All provided, no warning: {:?}", warns);
    }

    // ── Duplicate assignment ─────────────────────────────────

    #[test]
    fn duplicate_field_warns() {
        let diags = query_diagnostics(
            "CREATE user SET name = 'Alice', name = 'Bob', age = 30, email = 'a@b.com', active = true"
        );
        let warns: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::DuplicateFieldAssignment)
            .collect();
        assert!(!warns.is_empty(), "Should warn on duplicate: {:?}", diags);
    }

    // ── CONTENT ──────────────────────────────────────────────

    #[test]
    fn content_valid_no_error() {
        let diags = query_diagnostics(
            "CREATE user CONTENT { name: 'Alice', age: 30, email: 'a@b.com', active: true, tags: ['x'] }"
        );
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Valid CONTENT: {:?}", errors);
    }

    #[test]
    fn content_param_no_type_error() {
        let diags = query_diagnostics("CREATE user CONTENT $data");
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.is_error() && d.code == Code::IncompatibleAssignment)
            .collect();
        assert!(errors.is_empty(), "$param should not type-error: {:?}", errors);
    }
}
