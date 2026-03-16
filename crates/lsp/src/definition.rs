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

    let mut current = node;
    loop {
        let Some(parent) = current.parent() else { break; };

        match parent.kind() {
            // Table references
            "from_clause" | "create_target" | "relate_subject"
                if current.kind() == "identifier" =>
            {
                return find_definition(ctx, &text, DefKind::Table, None, source, uri, schema_sources);
            }
            "graph_path" | "graph_predicate" if current.kind() == "identifier" => {
                return find_definition(ctx, &text, DefKind::Table, None, source, uri, schema_sources);
            }
            // Field references in clauses
            "field_assignment" | "where_clause" | "select_clause"
                if current.kind() == "identifier" =>
            {
                let table = find_statement_table(&parent, source);
                return find_definition(ctx, &text, DefKind::Field, table.as_deref(), source, uri, schema_sources);
            }
            // Variables
            _ if current.kind() == "variable_name" || current.kind() == "variable" => {
                let var = if text.starts_with('$') { text.clone() } else { format!("${text}") };
                if let Some(binding) = ctx.scope.lookup(&var) {
                    return resolve_span(binding.span, "LET", &var, None, source, uri, schema_sources);
                }
            }
            // Functions
            _ if current.kind() == "custom_function_name" => {
                return find_definition(ctx, &text, DefKind::Function, None, source, uri, schema_sources);
            }
            _ => {}
        }

        // Object key (CONTENT clause, object literal) → field definition
        if current.kind() == "object_key"
            || (current.kind() == "identifier" && parent.kind() == "object_property")
        {
            let table = find_statement_table(&parent, source);
            return find_definition(ctx, &text, DefKind::Field, table.as_deref(), source, uri, schema_sources);
        }

        // DML statement targets
        if current.kind() == "identifier" {
            match parent.kind() {
                "update_statement" | "delete_statement" | "upsert_statement" => {
                    if is_target_identifier(&parent, &current) {
                        return find_definition(ctx, &text, DefKind::Table, None, source, uri, schema_sources);
                    }
                }
                _ => {}
            }
        }

        current = parent;
    }

    // Fallback
    if ctx.has_table(&text) {
        return find_definition(ctx, &text, DefKind::Table, None, source, uri, schema_sources);
    }
    if ctx.get_function(&text).is_some() {
        return find_definition(ctx, &text, DefKind::Function, None, source, uri, schema_sources);
    }

    None
}

#[derive(Clone, Copy)]
enum DefKind { Table, Field, Function }

/// Find a definition by looking up the span in context, then resolving it
/// against the current file or schema files.
fn find_definition(
    ctx: &Context,
    name: &str,
    kind: DefKind,
    table: Option<&str>,
    source: &str,
    uri: &Url,
    schema_sources: &[SchemaSource],
) -> Option<GotoDefinitionResponse> {
    match kind {
        DefKind::Table => {
            let span = ctx.get_table(name)?.span;
            resolve_span(span, "DEFINE TABLE", name, None, source, uri, schema_sources)
        }
        DefKind::Field => {
            // Try specific table first, then all tables
            if let Some(tbl) = table {
                if let Some(field) = ctx.get_field(tbl, name) {
                    return resolve_span(field.span, "DEFINE FIELD", name, Some(tbl), source, uri, schema_sources);
                }
            }
            for tbl_name in ctx.table_names() {
                if let Some(field) = ctx.get_field(tbl_name, name) {
                    return resolve_span(field.span, "DEFINE FIELD", name, Some(tbl_name), source, uri, schema_sources);
                }
            }
            None
        }
        DefKind::Function => {
            let span = ctx.get_function(name)?.span;
            resolve_span(span, "DEFINE FUNCTION", name, None, source, uri, schema_sources)
        }
    }
}

/// Resolve a span to an LSP Location, checking current file then schema files.
fn resolve_span(
    span: Span,
    define_keyword: &str,
    name: &str,
    table: Option<&str>,
    current_source: &str,
    current_uri: &Url,
    schema_sources: &[SchemaSource],
) -> Option<GotoDefinitionResponse> {
    let start = span.start as usize;
    let end = span.end as usize;

    // Try current file
    if end <= current_source.len() && start < end {
        let span_text = &current_source[start..end];
        if is_definition_text(span_text, name) {
            let range = byte_range_to_lsp(current_source, start, end);
            return Some(GotoDefinitionResponse::Scalar(Location {
                uri: current_uri.clone(),
                range,
            }));
        }
    }

    // Try schema files — first by span offset, then by text search
    for schema in schema_sources {
        if end <= schema.text.len() && start < end {
            let span_text = &schema.text[start..end];
            if is_definition_text(span_text, name) {
                let range = byte_range_to_lsp(&schema.text, start, end);
                return Some(GotoDefinitionResponse::Scalar(Location {
                    uri: schema.uri.clone(),
                    range,
                }));
            }
        }

        // Text search with table context for precision
        let search = match (define_keyword, table) {
            ("DEFINE FIELD", Some(tbl)) => format!("DEFINE FIELD {} ON {}", name, tbl),
            _ => format!("{} {}", define_keyword, name),
        };
        if let Some(pos) = schema.text.find(&search) {
            let stmt_end = schema.text[pos..]
                .find(';')
                .map(|i| pos + i + 1)
                .unwrap_or(pos + search.len());
            let range = byte_range_to_lsp(&schema.text, pos, stmt_end);
            return Some(GotoDefinitionResponse::Scalar(Location {
                uri: schema.uri.clone(),
                range,
            }));
        }
    }

    None
}

/// Check if span text is a DEFINE/LET statement, not arbitrary code.
fn is_definition_text(text: &str, name: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.starts_with("--") || trimmed.starts_with("//") {
        return false;
    }
    if !trimmed.contains(name) {
        return false;
    }
    let upper = trimmed.to_uppercase();
    upper.starts_with("DEFINE ") || upper.starts_with("LET ")
}
