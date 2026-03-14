use tree_sitter::Node;

use crate::context::Context;
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{child_by_kind, find_all, node_text};
use crate::resolve::resolve_expr;
use crate::span::Span;
use crate::types::{Kind, KindExt};

/// Analyze a LIVE SELECT statement and return Kind::Uuid.
///
/// LIVE SELECT returns a subscription UUID, but we still analyze the inner
/// query for diagnostics (table existence, WHERE clause type, FETCH validity).
///
/// Grammar:
///   live_select_statement = keyword_live + choice(select_statement, live_select_diff_statement)
///   live_select_diff_statement = keyword_select + keyword_diff + keyword_from + values
///                                + optional(where_clause) + optional(fetch_clause)
pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    let kind = node.kind();

    if kind == "live_select_statement" {
        // The live_select_statement wraps either a select_statement or live_select_diff_statement
        if let Some(inner_select) = child_by_kind(node, "select_statement") {
            // Delegate full analysis to the SELECT analyzer for diagnostics
            super::select::analyze(&inner_select, source, ctx);
        } else if let Some(diff_stmt) = child_by_kind(node, "live_select_diff_statement") {
            analyze_diff(&diff_stmt, source, ctx);
        }
    } else if kind == "live_select_diff_statement" {
        analyze_diff(node, source, ctx);
    }

    // LIVE SELECT always returns a UUID (the subscription ID)
    Kind::Uuid
}

/// Analyze a LIVE SELECT DIFF statement for diagnostics.
///
/// Structure: keyword_select, keyword_diff, keyword_from, value+, where_clause?, fetch_clause?
fn analyze_diff(node: &Node, source: &str, ctx: &mut Context) {
    let table = extract_diff_table(node, source);
    let table_ref = table.as_deref();

    // Validate table exists (strict mode)
    if let Some(tbl) = table_ref {
        if ctx.strict && !ctx.has_table(tbl) {
            let span = Span::from_node(node);
            ctx.emit(Diagnostic::error(
                span,
                Code::TableNotFound,
                format!("table `{}` not defined", tbl),
            ));
        }
    }

    // Analyze WHERE clause
    let where_clauses = find_all(node, "where_clause");
    for wc in &where_clauses {
        analyze_where(wc, source, ctx, table_ref);
    }

    // Validate FETCH fields
    let fetch_fields = extract_fetch_fields(node, source);
    if !fetch_fields.is_empty() {
        if let Some(tbl) = table_ref {
            let fetch_nodes = find_all(node, "fetch_clause");
            if let Some(fetch_node) = fetch_nodes.first() {
                for field in &fetch_fields {
                    if let Some(field_def) = ctx.get_field(tbl, field) {
                        if let Some(ref typ) = field_def.typ {
                            if !typ.is_record() && *typ != Kind::Any {
                                let span = Span::from_node(fetch_node);
                                ctx.emit(Diagnostic::warning(
                                    span,
                                    Code::TypeMismatch,
                                    format!(
                                        "FETCH field `{}` is not a record type, got `{}`",
                                        field, typ
                                    ),
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Extract the table name from a LIVE SELECT DIFF statement.
///
/// The table is the value(s) after keyword_from.
fn extract_diff_table(node: &Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    let mut past_from = false;
    for child in node.children(&mut cursor) {
        if child.kind() == "keyword_from" {
            past_from = true;
        } else if past_from {
            match child.kind() {
                "identifier" => return Some(node_text(&child, source).to_string()),
                "value" | "base_value" => {
                    let idents = find_all(&child, "identifier");
                    if let Some(ident) = idents.first() {
                        return Some(node_text(ident, source).to_string());
                    }
                }
                k if k.starts_with("keyword_") => break,
                _ => {}
            }
        }
    }
    None
}

/// Analyze a WHERE clause — condition must be boolean.
fn analyze_where(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "keyword_where" {
            let typ = resolve_expr(&child, source, ctx, table);
            if typ != Kind::Bool && typ != Kind::Any {
                let span = Span::from_node(&child);
                ctx.emit(Diagnostic::error(
                    span,
                    Code::TypeMismatch,
                    format!("WHERE clause should be bool, got `{}`", typ),
                ));
            }
        }
    }
}

/// Extract field names from FETCH clause.
fn extract_fetch_fields(node: &Node, source: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let fetch_nodes = find_all(node, "fetch_clause");
    for fetch in &fetch_nodes {
        for ident in find_all(fetch, "identifier") {
            fields.push(node_text(&ident, source).to_string());
        }
    }
    fields
}

#[cfg(test)]
mod tests {
    #[test]
    fn live_select_validates_table_strict() {
        let result = crate::analyze_strict(
            r#"
            LIVE SELECT * FROM nonexistent;
        "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("nonexistent"))
            .collect();
        assert!(!errors.is_empty());
    }

    #[test]
    fn live_select_where_bool_check() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            LIVE SELECT * FROM user WHERE "not a bool";
        "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("WHERE"))
            .collect();
        assert!(!warnings.is_empty());
    }

    #[test]
    fn live_select_valid_no_errors() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            LIVE SELECT * FROM user WHERE name = 'test';
        "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty());
    }
}
