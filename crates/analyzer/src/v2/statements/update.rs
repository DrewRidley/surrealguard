//! UPDATE statement analysis.
//!
//! Handles: target table, SET clause (field/type checking, compound operators),
//! CONTENT clause, MERGE clause, WHERE clause validation,
//! RETURN clause, ONLY modifier, duplicate assignments.
//!
//! Result type:
//! - UPDATE table SET ... → array<table_type>
//! - UPDATE ONLY table SET ... → table_type

use std::collections::HashSet;
use tree_sitter::Node;

use crate::context::{Context, SchemaMode};
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{child_by_kind, find_all, node_text};
use crate::span::Span;
use crate::types::{Kind, KindExt};
use crate::v2::expr;

/// Analyze an UPDATE statement. Returns the result type.
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

    // Process data clauses
    process_set_clause(node, source, ctx, table.as_deref());
    process_content_clause(node, source, ctx, table.as_deref());

    // Validate WHERE
    if let Some(where_clause) = child_by_kind(node, "where_clause") {
        validate_where(&where_clause, source, ctx, table.as_deref());
    }

    // Build result type
    let table_type = table.as_deref()
        .and_then(|tbl| ctx.build_table_type(tbl))
        .unwrap_or(Kind::Any);

    let result = process_return_clause(node, &table_type);

    if is_only { result } else { Kind::Array(Box::new(result), None) }
}

// ── Target table ─────────────────────────────────────────────

fn resolve_target_table(node: &Node, source: &str) -> Option<String> {
    // UPDATE target is a direct child identifier (before any clause)
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();
    for child in &children {
        if child.kind().ends_with("_clause") || child.kind() == "set_clause" {
            break;
        }
        if child.kind() == "identifier" {
            return Some(node_text(child, source).to_string());
        }
        if child.kind() == "value" || child.kind() == "base_value" {
            for ident in find_all(child, "identifier") {
                return Some(node_text(&ident, source).to_string());
            }
            for rid in find_all(child, "record_id") {
                let text = node_text(&rid, source);
                if let Some(t) = text.split(':').next() {
                    return Some(t.to_string());
                }
            }
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
        if child.kind() == "value" || child.kind() == "base_value" {
            if let Some(ident) = find_all(child, "identifier").first() {
                return Span::from_node(ident);
            }
        }
    }
    Span::from_node(node)
}

// ── SET clause ───────────────────────────────────────────────

fn process_set_clause(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    let mut seen = HashSet::new();

    for assignment in find_all(node, "field_assignment") {
        let idents = find_all(&assignment, "identifier");
        let Some(field_ident) = idents.first() else { continue };
        let field_name = node_text(field_ident, source);

        // Duplicate check
        if !seen.insert(field_name.to_string()) {
            ctx.emit(Diagnostic::warning(
                Span::from_node(field_ident),
                Code::DuplicateFieldAssignment,
                format!("field `{}` is assigned multiple times", field_name),
            ));
        }

        let Some(tbl) = table else { continue };

        let is_schemafull = ctx.get_table(tbl)
            .map(|t| t.schema_mode == SchemaMode::Schemafull)
            .unwrap_or(false);

        let field_info = ctx.get_field(tbl, field_name)
            .map(|f| (f.readonly, f.is_computed, f.typ.clone()));

        if field_info.is_none() && is_schemafull {
            ctx.emit(Diagnostic::warning(
                Span::from_node(field_ident),
                Code::UndefinedFieldOnSchemafull,
                format!("field `{}` is not defined on table `{}`", field_name, tbl),
            ));
            continue;
        }

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

            // Type check the value
            if let Some(ref expected) = typ {
                let val = find_assignment_value(&assignment);
                if let Some(val_node) = val {
                    // Detect compound operator (+=, -=)
                    let operator = find_operator(&assignment, source);
                    let actual = expr::resolve_simple(&val_node, source, ctx, Some(tbl));

                    if !actual.is_any() {
                        let compatible = match operator.as_deref() {
                            Some("+=") => is_compound_add_compatible(expected, &actual),
                            Some("-=") => is_compound_sub_compatible(expected, &actual),
                            _ => crate::types::is_assignable(expected, &actual),
                        };
                        if !compatible {
                            ctx.emit(Diagnostic::error(
                                Span::from_node(&val_node),
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

fn find_assignment_value<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let children: Vec<Node<'a>> = node.named_children(&mut cursor).collect();
    children.into_iter().rev().find(|c| {
        c.kind() != "identifier" && c.kind() != "operator"
    })
}

fn find_operator(node: &Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();
    children.iter()
        .find(|c| c.kind() == "operator" || c.kind() == "=")
        .map(|c| node_text(c, source).to_string())
}

fn is_compound_add_compatible(field_type: &Kind, value_type: &Kind) -> bool {
    match field_type {
        Kind::Int | Kind::Float | Kind::Number | Kind::Decimal => value_type.is_numeric(),
        Kind::String => matches!(value_type, Kind::String),
        Kind::Array(_, _) => true, // += appends to array
        _ => false,
    }
}

fn is_compound_sub_compatible(field_type: &Kind, value_type: &Kind) -> bool {
    match field_type {
        Kind::Int | Kind::Float | Kind::Number | Kind::Decimal => value_type.is_numeric(),
        Kind::Array(_, _) => true, // -= removes from array
        _ => false,
    }
}

// ── CONTENT clause ───────────────────────────────────────────

fn process_content_clause(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    let Some(content) = child_by_kind(node, "content_clause") else { return };
    let Some(tbl) = table else { return };

    for obj in find_all(&content, "object") {
        for prop in find_all(&obj, "object_property") {
            let keys = find_all(&prop, "object_key");
            let ident_keys = find_all(&prop, "identifier");
            let key = keys.first().or(ident_keys.first()).copied();
            if let Some(key_node) = key {
                let key_name = node_text(&key_node, source);
                let expected_type = ctx.get_field(tbl, key_name).and_then(|f| f.typ.clone());
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

// ── WHERE clause ─────────────────────────────────────────────

fn validate_where(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();
    for child in &children {
        if child.kind() != "keyword_where" {
            let typ = expr::resolve_simple(child, source, ctx, table);
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

// ── RETURN clause ────────────────────────────────────────────

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
                DEFINE FIELD email ON user TYPE string;
                DEFINE FIELD active ON user TYPE bool;
                DEFINE FIELD tags ON user TYPE array<string>;
                DEFINE FIELD status ON user TYPE string READONLY;

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
    fn returns_array_of_table_type() {
        let typ = query_type("UPDATE user SET name = 'Bob'");
        assert!(matches!(&typ, Kind::Array(_, _)), "Expected array, got: {:?}", typ);
    }

    // ── Compatible types ─────────────────────────────────────

    #[test]
    fn set_compatible_no_error() {
        let diags = query_diagnostics("UPDATE user SET name = 'Bob', age = 25");
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Compatible types: {:?}", errors);
    }

    // ── Type mismatch ────────────────────────────────────────

    #[test]
    fn set_type_mismatch() {
        let diags = query_diagnostics("UPDATE user SET age = 'twenty'");
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::IncompatibleAssignment)
            .collect();
        assert!(!errors.is_empty(), "Expected mismatch: {:?}", diags);
    }

    #[test]
    fn content_type_mismatch() {
        let diags = query_diagnostics("UPDATE user CONTENT { age: 'bad', name: 'ok' }");
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::IncompatibleAssignment)
            .collect();
        assert!(!errors.is_empty(), "Expected CONTENT mismatch: {:?}", diags);
    }

    // ── Undefined field ──────────────────────────────────────

    #[test]
    fn undefined_field_schemafull() {
        let diags = query_diagnostics("UPDATE user SET ghost = 'boo'");
        let warns: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::UndefinedFieldOnSchemafull)
            .collect();
        assert!(!warns.is_empty(), "Should warn on 'ghost': {:?}", diags);
    }

    #[test]
    fn undefined_field_schemaless_ok() {
        let diags = query_diagnostics("UPDATE flex SET anything = 'goes'");
        let warns: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::UndefinedFieldOnSchemafull)
            .collect();
        assert!(warns.is_empty(), "Schemaless allows any: {:?}", warns);
    }

    // ── Readonly ─────────────────────────────────────────────

    #[test]
    fn readonly_field_errors() {
        let diags = query_diagnostics("UPDATE user SET status = 'inactive'");
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::ReadonlyAssignment)
            .collect();
        assert!(!errors.is_empty(), "Should error on readonly: {:?}", diags);
    }

    // ── Duplicate assignment ─────────────────────────────────

    #[test]
    fn duplicate_field_warns() {
        let diags = query_diagnostics("UPDATE user SET name = 'A', name = 'B'");
        let warns: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::DuplicateFieldAssignment)
            .collect();
        assert!(!warns.is_empty(), "Should warn on dup: {:?}", diags);
    }

    // ── Compound operators ───────────────────────────────────

    #[test]
    fn plus_eq_numeric_valid() {
        let diags = query_diagnostics("UPDATE user SET age += 1");
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::IncompatibleAssignment)
            .collect();
        assert!(errors.is_empty(), "int += int should be valid: {:?}", errors);
    }

    #[test]
    fn plus_eq_string_to_int_invalid() {
        let diags = query_diagnostics("UPDATE user SET age += 'one'");
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::IncompatibleAssignment)
            .collect();
        assert!(!errors.is_empty(), "int += string should error: {:?}", diags);
    }

    #[test]
    fn plus_eq_string_concat_valid() {
        let diags = query_diagnostics("UPDATE user SET name += ' Jr.'");
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::IncompatibleAssignment)
            .collect();
        assert!(errors.is_empty(), "string += string should be valid: {:?}", errors);
    }

    // ── WHERE clause ─────────────────────────────────────────

    #[test]
    fn where_with_valid_condition() {
        let diags = query_diagnostics("UPDATE user SET name = 'Bob' WHERE active = true");
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Valid WHERE: {:?}", errors);
    }
}
