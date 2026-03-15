//! Go-to-definition — resolves the definition location of the symbol under
//! the cursor using tree-sitter AST walking and the analyzer's Context.

use tower_lsp::lsp_types::{GotoDefinitionResponse, Location, Position, Url};

use surrealguard_analyzer::{self as sg, Context, Span};

use crate::hover::{find_statement_table, is_target_identifier};
use crate::text::{byte_range_to_lsp, position_to_offset};
use crate::workspace::SchemaSource;

/// Resolve go-to-definition for a document at a given position.
pub fn resolve(
    source: &str,
    position: Position,
    ctx: &Context,
    uri: &Url,
    schema_sources: &[SchemaSource],
) -> Option<GotoDefinitionResponse> {
    let offset = position_to_offset(source, position);
    let tree = sg::parse(source).ok()?;
    let root = tree.root_node();
    let node = root.descendant_for_byte_range(offset, offset)?;

    let text = node.utf8_text(source.as_bytes()).ok()?.trim().to_string();
    if text.is_empty() {
        return None;
    }

    // Walk up the tree to determine context
    let mut current = node;
    loop {
        let Some(parent) = current.parent() else {
            break;
        };

        match parent.kind() {
            "from_clause" | "create_target" | "relate_subject"
                if current.kind() == "identifier" =>
            {
                return resolve_span(ctx.get_table(&text).map(|t| t.span), &text, "DEFINE TABLE", source, uri, schema_sources);
            }
            "graph_path" | "graph_predicate" if current.kind() == "identifier" => {
                return resolve_span(ctx.get_table(&text).map(|t| t.span), &text, "DEFINE TABLE", source, uri, schema_sources);
            }
            "field_assignment" | "where_clause" | "select_clause"
                if current.kind() == "identifier" =>
            {
                let table = find_statement_table(&parent, source);
                if let Some(tbl) = &table {
                    if let Some(field) = ctx.get_field(tbl, &text) {
                        return resolve_span(Some(field.span), &text, "DEFINE FIELD", source, uri, schema_sources);
                    }
                }
                // Try all tables
                for tbl_name in ctx.table_names() {
                    if let Some(field) = ctx.get_field(tbl_name, &text) {
                        return resolve_span(Some(field.span), &text, "DEFINE FIELD", source, uri, schema_sources);
                    }
                }
            }
            _ if current.kind() == "variable_name" || current.kind() == "variable" => {
                let var = if text.starts_with('$') { text.clone() } else { format!("${text}") };
                if let Some(binding) = ctx.scope.lookup(&var) {
                    return resolve_span(Some(binding.span), &var, "LET", source, uri, schema_sources);
                }
            }
            _ if current.kind() == "custom_function_name" => {
                if let Some(func) = ctx.get_function(&text) {
                    return resolve_span(Some(func.span), &text, "DEFINE FUNCTION", source, uri, schema_sources);
                }
            }
            _ => {}
        }

        // DML statement targets
        if current.kind() == "identifier" {
            match parent.kind() {
                "update_statement" | "delete_statement" | "upsert_statement" => {
                    if is_target_identifier(&parent, &current) {
                        return resolve_span(ctx.get_table(&text).map(|t| t.span), &text, "DEFINE TABLE", source, uri, schema_sources);
                    }
                }
                _ => {}
            }
        }

        current = parent;
    }

    // Fallback: try as table, then function
    if let Some(table) = ctx.get_table(&text) {
        return resolve_span(Some(table.span), &text, "DEFINE TABLE", source, uri, schema_sources);
    }
    if let Some(func) = ctx.get_function(&text) {
        return resolve_span(Some(func.span), &text, "DEFINE FUNCTION", source, uri, schema_sources);
    }

    None
}

/// Resolve a definition span to a Location, checking both the current file
/// and cross-file schema sources.
fn resolve_span(
    span: Option<Span>,
    name: &str,
    define_keyword: &str,
    current_source: &str,
    current_uri: &Url,
    schema_sources: &[SchemaSource],
) -> Option<GotoDefinitionResponse> {
    let span = span?;
    let start = span.start as usize;
    let end = span.end as usize;

    // Check if span is valid in the current file and points to actual definition text
    if end <= current_source.len() {
        let span_text = &current_source[start..end];
        if span_text.contains(name) && !span_text.trim_start().starts_with("--") {
            let range = byte_range_to_lsp(current_source, start, end);
            return Some(GotoDefinitionResponse::Scalar(Location {
                uri: current_uri.clone(),
                range,
            }));
        }
    }

    // Search schema files for the definition
    for schema in schema_sources {
        if end <= schema.text.len() {
            let span_text = &schema.text[start..end];
            if span_text.contains(name) && !span_text.trim_start().starts_with("--") {
                let range = byte_range_to_lsp(&schema.text, start, end);
                return Some(GotoDefinitionResponse::Scalar(Location {
                    uri: schema.uri.clone(),
                    range,
                }));
            }
        }
        // Also try searching the entire file for the DEFINE statement
        if let Some(pos) = schema.text.find(&format!("{} {}", define_keyword, name)) {
            // Find the end of the statement (next semicolon)
            let stmt_end = schema.text[pos..].find(';').map(|i| pos + i + 1).unwrap_or(pos + name.len());
            let range = byte_range_to_lsp(&schema.text, pos, stmt_end);
            return Some(GotoDefinitionResponse::Scalar(Location {
                uri: schema.uri.clone(),
                range,
            }));
        }
    }

    None
}
