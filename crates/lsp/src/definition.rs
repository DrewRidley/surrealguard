//! Go-to-definition — resolves the definition location of the symbol under
//! the cursor using tree-sitter AST walking and the analyzer's Context.

use tower_lsp::lsp_types::{GotoDefinitionResponse, Location, Position, Url};

use surrealguard_analyzer::{self as sg, Context};

use crate::hover::{find_statement_table, is_target_identifier};
use crate::text::{byte_range_to_lsp, position_to_offset};

/// Resolve go-to-definition for a document at a given position.
///
/// Returns a `GotoDefinitionResponse::Scalar` pointing to the definition
/// of the symbol under the cursor, or `None` if no definition is found.
pub fn resolve(
    source: &str,
    position: Position,
    ctx: &Context,
    uri: &Url,
) -> Option<GotoDefinitionResponse> {
    let offset = position_to_offset(source, position);
    let tree = sg::parse(source).ok()?;
    let root = tree.root_node();
    let node = root.descendant_for_byte_range(offset, offset)?;

    let text = node.utf8_text(source.as_bytes()).ok()?.trim().to_string();
    if text.is_empty() {
        return None;
    }

    // Walk up the tree to understand context
    let mut current = node;
    loop {
        let Some(parent) = current.parent() else {
            break;
        };

        match parent.kind() {
            // Identifier in FROM clause -> table reference
            "from_clause" if current.kind() == "identifier" => {
                return table_definition(&text, ctx, source, uri);
            }
            // Identifier in graph path -> relation or target table
            "graph_predicate" | "graph_path" if current.kind() == "identifier" => {
                return table_definition(&text, ctx, source, uri);
            }
            // Identifier in create_target -> table
            "create_target" if current.kind() == "identifier" => {
                return table_definition(&text, ctx, source, uri);
            }
            // Identifier in relate_subject -> table
            "relate_subject" if current.kind() == "identifier" => {
                return table_definition(&text, ctx, source, uri);
            }
            // Field in SET/WHERE/SELECT -> field definition
            "field_assignment" if current.kind() == "identifier" => {
                let table = find_statement_table(&parent, source);
                return field_definition(&text, table.as_deref(), ctx, source, uri);
            }
            "where_clause" | "select_clause" if current.kind() == "identifier" => {
                let table = find_statement_table(&parent, source);
                return field_definition(&text, table.as_deref(), ctx, source, uri);
            }
            // Variable reference
            _ if current.kind() == "variable_name" || current.kind() == "variable" => {
                return variable_definition(&text, ctx, source, uri);
            }
            _ => {}
        }

        // DML statement target identifiers (UPDATE user, DELETE user)
        if current.kind() == "identifier" {
            match parent.kind() {
                "update_statement" | "delete_statement" | "upsert_statement" => {
                    if is_target_identifier(&parent, &current) {
                        return table_definition(&text, ctx, source, uri);
                    }
                }
                _ => {}
            }
        }

        current = parent;
    }

    // Fallback: try as table name
    if ctx.has_table(&text) {
        return table_definition(&text, ctx, source, uri);
    }

    // Try as custom function (fn::name)
    if text.starts_with("fn::") || ctx.get_function(&text).is_some() {
        return function_definition(&text, ctx, source, uri);
    }

    None
}

/// Resolve the definition location of a table.
fn table_definition(
    name: &str,
    ctx: &Context,
    source: &str,
    uri: &Url,
) -> Option<GotoDefinitionResponse> {
    let table = ctx.get_table(name)?;
    let range = byte_range_to_lsp(source, table.span.start as usize, table.span.end as usize);
    Some(GotoDefinitionResponse::Scalar(Location {
        uri: uri.clone(),
        range,
    }))
}

/// Resolve the definition location of a field.
fn field_definition(
    name: &str,
    table: Option<&str>,
    ctx: &Context,
    source: &str,
    uri: &Url,
) -> Option<GotoDefinitionResponse> {
    // Try specific table first
    if let Some(tbl) = table {
        if let Some(field) = ctx.get_field(tbl, name) {
            let range =
                byte_range_to_lsp(source, field.span.start as usize, field.span.end as usize);
            return Some(GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range,
            }));
        }
    }

    // Try all tables — return the first match
    for tbl_name in ctx.table_names() {
        if let Some(field) = ctx.get_field(tbl_name, name) {
            let range =
                byte_range_to_lsp(source, field.span.start as usize, field.span.end as usize);
            return Some(GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range,
            }));
        }
    }

    None
}

/// Resolve the definition location of a variable.
fn variable_definition(
    name: &str,
    ctx: &Context,
    source: &str,
    uri: &Url,
) -> Option<GotoDefinitionResponse> {
    let var_name = if name.starts_with('$') {
        name.to_string()
    } else {
        format!("${name}")
    };

    let binding = ctx.scope.lookup(&var_name)?;
    let range =
        byte_range_to_lsp(source, binding.span.start as usize, binding.span.end as usize);
    Some(GotoDefinitionResponse::Scalar(Location {
        uri: uri.clone(),
        range,
    }))
}

/// Resolve the definition location of a custom function (fn::name).
fn function_definition(
    name: &str,
    ctx: &Context,
    source: &str,
    uri: &Url,
) -> Option<GotoDefinitionResponse> {
    let func = ctx.get_function(name)?;
    let range = byte_range_to_lsp(source, func.span.start as usize, func.span.end as usize);
    Some(GotoDefinitionResponse::Scalar(Location {
        uri: uri.clone(),
        range,
    }))
}
