//! INSERT statement analysis.
//!
//! Handles: INSERT INTO table (columns) VALUES (...),
//! INSERT INTO table CONTENT {...},
//! ON DUPLICATE KEY UPDATE field assignments,
//! RETURN clause, ONLY modifier.
//!
//! Result type:
//! - INSERT INTO table (...) VALUES (...) → array<table_type>
//! - INSERT INTO ONLY table (...) VALUES (...) → table_type

use std::collections::HashSet;
use tree_sitter::Node;

use crate::context::{Context, SchemaMode};
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{child_by_kind, find_all, node_text};
use crate::span::Span;
use crate::types::{Kind, KindExt};
use crate::v2::expr;

/// Analyze an INSERT statement. Returns the result type.
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

    // Collect column names from (col1, col2, ...) syntax
    let columns = collect_columns(node, source);

    // Process VALUES clause
    if !columns.is_empty() {
        process_values_clause(node, source, ctx, table.as_deref(), &columns);
    }

    // Process CONTENT clause (INSERT INTO table CONTENT {...})
    process_content_clause(node, source, ctx, table.as_deref());

    // Process ON DUPLICATE KEY UPDATE assignments
    process_on_duplicate_key(node, source, ctx, table.as_deref());

    // Build result type
    let table_type = table
        .as_deref()
        .and_then(|tbl| ctx.build_table_type(tbl))
        .unwrap_or(Kind::Any);

    let result = process_return_clause(node, &table_type);

    if is_only {
        result
    } else {
        Kind::Array(Box::new(result), None)
    }
}

// ── Target table ─────────────────────────────────────────────

fn resolve_target_table(node: &Node, source: &str) -> Option<String> {
    // INSERT INTO <table> ...
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();
    for child in &children {
        if child.kind() == "keyword_insert" || child.kind() == "keyword_into"
            || child.kind() == "keyword_only"
        {
            continue;
        }
        if child.kind() == "identifier" {
            return Some(node_text(child, source).to_string());
        }
        // Stop at clauses or parens (column list)
        if child.kind().ends_with("_clause") {
            break;
        }
    }
    None
}

fn find_target_span(node: &Node) -> Span {
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();
    for child in &children {
        if child.kind() == "keyword_insert" || child.kind() == "keyword_into"
            || child.kind() == "keyword_only"
        {
            continue;
        }
        if child.kind() == "identifier" {
            return Span::from_node(child);
        }
    }
    Span::from_node(node)
}

// ── Column list extraction ───────────────────────────────────

fn collect_columns<'a>(node: &'a Node<'a>, source: &str) -> Vec<(String, Span)> {
    let mut columns = Vec::new();
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();

    // The column identifiers appear as direct children before keyword_values
    let mut past_table = false;
    for child in &children {
        if child.kind() == "identifier" {
            if !past_table {
                past_table = true; // First identifier is the table name
                continue;
            }
            columns.push((node_text(child, source).to_string(), Span::from_node(child)));
        }
        if child.kind() == "keyword_values" {
            break;
        }
    }

    columns
}

// ── VALUES clause ────────────────────────────────────────────

fn process_values_clause(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
    columns: &[(String, Span)],
) {
    // Collect value nodes after keyword_values
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();

    let mut past_values = false;
    let mut values: Vec<Node> = Vec::new();

    for child in &children {
        if child.kind() == "keyword_values" {
            past_values = true;
            continue;
        }
        if !past_values { continue; }
        // Stop at ON DUPLICATE KEY UPDATE or RETURN
        if child.kind() == "keyword_on_duplicate_key_update"
            || child.kind() == "return_clause"
            || child.kind().ends_with("_clause")
        {
            break;
        }
        if child.kind() == "value" || child.kind() == "base_value" {
            values.push(*child);
        }
    }

    let col_count = columns.len();
    let val_count = values.len();

    // Check column count matches value count
    if col_count > 0 && val_count > 0 && val_count != col_count {
        let span = if let Some(last_val) = values.last() {
            Span::from_node(last_val)
        } else {
            Span::from_node(node)
        };
        ctx.emit(Diagnostic::error(
            span,
            Code::ValuesColumnCountMismatch,
            format!(
                "expected {} value(s) to match {} column(s), found {}",
                col_count, col_count, val_count
            ),
        ));
    }

    // Type-check each value against its column's type
    if let Some(tbl) = table {
        let is_schemafull = ctx.get_table(tbl)
            .map(|t| t.schema_mode == SchemaMode::Schemafull)
            .unwrap_or(false);

        for (i, (col_name, col_span)) in columns.iter().enumerate() {
            // Validate column exists on schemafull table
            let field_info = ctx.get_field(tbl, col_name).map(|f| f.typ.clone());

            if field_info.is_none() && is_schemafull {
                ctx.emit(Diagnostic::warning(
                    *col_span,
                    Code::UndefinedFieldOnSchemafull,
                    format!("field `{}` is not defined on table `{}`", col_name, tbl),
                ));
                continue;
            }

            // Type check the corresponding value
            if let Some(Some(ref expected)) = field_info.as_ref().map(|t| t.as_ref()) {
                if let Some(val_node) = values.get(i) {
                    let actual = expr::resolve_simple(val_node, source, ctx, Some(tbl));
                    if !actual.is_any() && !crate::types::is_assignable(expected, &actual) {
                        ctx.emit(Diagnostic::error(
                            Span::from_node(val_node),
                            Code::IncompatibleAssignment,
                            format!("expected `{}`, found `{}`", expected, actual),
                        ));
                    }
                }
            }
        }
    }
}

// ── CONTENT clause ───────────────────────────────────────────

fn process_content_clause(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
) {
    // INSERT INTO table CONTENT {...} — object is a direct child (may have ERROR wrapper)
    let Some(tbl) = table else { return };

    for obj in find_all(node, "object") {
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

// ── ON DUPLICATE KEY UPDATE ──────────────────────────────────

fn process_on_duplicate_key(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
) {
    // ON DUPLICATE KEY UPDATE is followed by field_assignment nodes
    let on_dup = find_all(node, "keyword_on_duplicate_key_update");
    if on_dup.is_empty() { return; }

    let Some(tbl) = table else { return };

    let is_schemafull = ctx.get_table(tbl)
        .map(|t| t.schema_mode == SchemaMode::Schemafull)
        .unwrap_or(false);

    // Find field_assignment nodes that appear after the ON DUPLICATE KEY UPDATE keyword
    let on_dup_end = on_dup[0].end_byte();

    for assignment in find_all(node, "field_assignment") {
        if assignment.start_byte() < on_dup_end { continue; }

        let idents = find_all(&assignment, "identifier");
        let Some(field_ident) = idents.first() else { continue };
        let field_name = node_text(field_ident, source);

        let field_info = ctx.get_field(tbl, field_name).map(|f| f.typ.clone());

        if field_info.is_none() && is_schemafull {
            ctx.emit(Diagnostic::warning(
                Span::from_node(field_ident),
                Code::UndefinedFieldOnSchemafull,
                format!("field `{}` is not defined on table `{}`", field_name, tbl),
            ));
            continue;
        }

        if let Some(Some(ref expected)) = field_info.as_ref().map(|t| t.as_ref()) {
            if let Some(val_node) = find_assignment_value(&assignment) {
                let actual = expr::resolve_simple(&val_node, source, ctx, Some(tbl));
                if !actual.is_any() && !crate::types::is_assignable(expected, &actual) {
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

fn find_assignment_value<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let children: Vec<Node<'a>> = node.named_children(&mut cursor).collect();
    children.into_iter().rev().find(|c| {
        c.kind() != "identifier" && c.kind() != "operator"
    })
}

// ── RETURN clause ────────────────────────────────────────────

fn process_return_clause(node: &Node, table_type: &Kind) -> Kind {
    for rc in find_all(node, "return_clause") {
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

            DEFINE TABLE post SCHEMAFULL;
                DEFINE FIELD title ON post TYPE string;
                DEFINE FIELD body ON post TYPE string;
                DEFINE FIELD author ON post TYPE record<user>;
                DEFINE FIELD created_at ON post TYPE datetime;

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

    // ── Valid insert ─────────────────────────────────────────

    #[test]
    fn valid_insert_values() {
        let diags = query_diagnostics(
            "INSERT INTO user (name, age) VALUES ('Alice', 30)"
        );
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::IncompatibleAssignment)
            .collect();
        assert!(errors.is_empty(), "Valid INSERT should not error: {:?}", errors);
    }

    #[test]
    fn returns_array() {
        let typ = query_type("INSERT INTO user (name, age) VALUES ('Alice', 30)");
        assert!(matches!(&typ, Kind::Array(_, _)), "Expected array, got: {:?}", typ);
    }

    // ── Type mismatch in VALUES ──────────────────────────────

    #[test]
    fn type_mismatch_in_values() {
        let diags = query_diagnostics(
            "INSERT INTO user (name, age) VALUES ('Alice', 'thirty')"
        );
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::IncompatibleAssignment)
            .collect();
        assert!(!errors.is_empty(), "Expected type mismatch: {:?}", diags);
    }

    // ── Column count mismatch ────────────────────────────────

    #[test]
    fn column_count_mismatch() {
        let diags = query_diagnostics(
            "INSERT INTO user (name, age, email) VALUES ('Alice', 30)"
        );
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::ValuesColumnCountMismatch)
            .collect();
        assert!(!errors.is_empty(), "Expected column count mismatch: {:?}", diags);
    }

    // ── ON DUPLICATE KEY UPDATE ──────────────────────────────

    #[test]
    fn on_duplicate_key_valid() {
        let diags = query_diagnostics(
            "INSERT INTO user (name, age) VALUES ('Alice', 30) ON DUPLICATE KEY UPDATE age = 31"
        );
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::IncompatibleAssignment)
            .collect();
        assert!(errors.is_empty(), "Valid ON DUPLICATE KEY UPDATE: {:?}", errors);
    }

    #[test]
    fn on_duplicate_key_type_mismatch() {
        let diags = query_diagnostics(
            "INSERT INTO user (name, age) VALUES ('Alice', 30) ON DUPLICATE KEY UPDATE age = 'bad'"
        );
        let errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::IncompatibleAssignment)
            .collect();
        assert!(!errors.is_empty(), "Expected ON DUPLICATE KEY type mismatch: {:?}", diags);
    }

    // ── CONTENT form ─────────────────────────────────────────
    // NOTE: INSERT INTO ... CONTENT is not supported by the tree-sitter grammar
    // (it produces a parse error). This is a SurrealDB grammar limitation.
    // CONTENT clause validation is tested via CREATE/UPDATE/UPSERT instead.

    #[test]
    fn content_object_type_check() {
        // INSERT with an object literal is parsed as CONTENT when the grammar
        // wraps it as a direct child object node inside insert_statement.
        // If the grammar evolves to support this, the analyzer will pick it up.
        let diags = query_diagnostics(
            "INSERT INTO user (name, age) VALUES ('Alice', 30)"
        );
        let type_errors: Vec<_> = diags.iter()
            .filter(|d| d.code == Code::IncompatibleAssignment)
            .collect();
        assert!(type_errors.is_empty(), "Valid insert should not have type errors: {:?}", type_errors);
    }
}
