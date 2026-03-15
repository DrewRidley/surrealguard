//! Document symbols — provides an outline of DEFINE statements in the file.
//!
//! Shows tables, fields, functions, indexes, events, params, and LET bindings
//! as a nested tree (fields nested under their tables).

use std::collections::BTreeMap;

use tower_lsp::lsp_types::{DocumentSymbol, Range, SymbolKind};

use crate::text::byte_range_to_lsp;

/// Collect document symbols from the source text.
///
/// Walks the tree-sitter AST to find all DEFINE statements and LET bindings,
/// then nests FIELD symbols under their TABLE symbols for a tree view.
#[allow(deprecated)] // DocumentSymbol::deprecated is deprecated but required by the struct
pub fn document_symbols(source: &str) -> Option<Vec<DocumentSymbol>> {
    let tree = surrealguard_analyzer::parse(source).ok()?;
    let root = tree.root_node();

    // Collect all symbols, separating tables and fields for nesting.
    let mut table_symbols: BTreeMap<String, DocumentSymbol> = BTreeMap::new();
    let mut table_order: Vec<String> = Vec::new();
    let mut top_level: Vec<DocumentSymbol> = Vec::new();
    let mut orphan_fields: Vec<DocumentSymbol> = Vec::new();

    collect_symbols(
        &root,
        source,
        &mut table_symbols,
        &mut table_order,
        &mut top_level,
        &mut orphan_fields,
    );

    // Build the final list: tables (with nested fields) first, then other top-level symbols.
    let mut result: Vec<DocumentSymbol> = Vec::new();

    for name in &table_order {
        if let Some(sym) = table_symbols.remove(name) {
            result.push(sym);
        }
    }

    // Add orphan fields (fields whose table wasn't defined in this file).
    result.extend(orphan_fields);

    // Add other top-level symbols (functions, params, let bindings).
    result.extend(top_level);

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

#[allow(deprecated)]
fn collect_symbols(
    node: &tree_sitter::Node,
    source: &str,
    table_symbols: &mut BTreeMap<String, DocumentSymbol>,
    table_order: &mut Vec<String>,
    top_level: &mut Vec<DocumentSymbol>,
    orphan_fields: &mut Vec<DocumentSymbol>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "define_table_statement" => {
                if let Some((name, sym)) = parse_define_table(&child, source) {
                    if !table_symbols.contains_key(&name) {
                        table_order.push(name.clone());
                    }
                    table_symbols.insert(name, sym);
                }
            }
            "define_field_statement" => {
                if let Some((table, sym)) = parse_define_field(&child, source) {
                    if let Some(table_sym) = table_symbols.get_mut(&table) {
                        let children = table_sym.children.get_or_insert_with(Vec::new);
                        children.push(sym);
                    } else {
                        orphan_fields.push(sym);
                    }
                }
            }
            "define_function_statement" => {
                if let Some(sym) = parse_define_function(&child, source) {
                    top_level.push(sym);
                }
            }
            "define_index_statement" => {
                if let Some((table, sym)) = parse_define_index(&child, source) {
                    if let Some(table_sym) = table_symbols.get_mut(&table) {
                        let children = table_sym.children.get_or_insert_with(Vec::new);
                        children.push(sym);
                    } else {
                        top_level.push(sym);
                    }
                }
            }
            "define_event_statement" => {
                if let Some((table, sym)) = parse_define_event(&child, source) {
                    if let Some(table_sym) = table_symbols.get_mut(&table) {
                        let children = table_sym.children.get_or_insert_with(Vec::new);
                        children.push(sym);
                    } else {
                        top_level.push(sym);
                    }
                }
            }
            "define_param_statement" => {
                if let Some(sym) = parse_define_param(&child, source) {
                    top_level.push(sym);
                }
            }
            "let_statement" => {
                if let Some(sym) = parse_let_statement(&child, source) {
                    top_level.push(sym);
                }
            }
            _ => {
                // Recurse into compound nodes (e.g., block, statement list).
                collect_symbols(
                    &child,
                    source,
                    table_symbols,
                    table_order,
                    top_level,
                    orphan_fields,
                );
            }
        }
    }
}

// ── Parsers for each DEFINE statement type ────────────────────

#[allow(deprecated)]
fn parse_define_table(node: &tree_sitter::Node, source: &str) -> Option<(String, DocumentSymbol)> {
    let name = find_name_after_keyword(node, source, &["TABLE"])?;
    let range = node_range(node, source);
    let selection_range = find_identifier_range(node, source).unwrap_or(range);

    let detail = extract_table_detail(node, source);

    Some((
        name.clone(),
        DocumentSymbol {
            name: format!("TABLE {}", name),
            detail,
            kind: SymbolKind::STRUCT,
            tags: None,
            deprecated: None,
            range,
            selection_range,
            children: None,
        },
    ))
}

#[allow(deprecated)]
fn parse_define_field(
    node: &tree_sitter::Node,
    source: &str,
) -> Option<(String, DocumentSymbol)> {
    let field_name = find_name_after_keyword(node, source, &["FIELD"])?;
    let table_name = find_on_table(node, source)?;
    let range = node_range(node, source);
    let selection_range = find_identifier_range(node, source).unwrap_or(range);

    let detail = extract_field_type(node, source);

    Some((
        table_name,
        DocumentSymbol {
            name: format!("FIELD {}", field_name),
            detail,
            kind: SymbolKind::FIELD,
            tags: None,
            deprecated: None,
            range,
            selection_range,
            children: None,
        },
    ))
}

#[allow(deprecated)]
fn parse_define_function(node: &tree_sitter::Node, source: &str) -> Option<DocumentSymbol> {
    // Function name can be in custom_function_name or identifier.
    let name = find_child_text(node, source, "custom_function_name")
        .or_else(|| find_name_after_keyword(node, source, &["FUNCTION"]))?;

    let range = node_range(node, source);
    let selection_range = find_node_range(node, source, "custom_function_name")
        .or_else(|| find_identifier_range(node, source))
        .unwrap_or(range);

    Some(DocumentSymbol {
        name: format!("FUNCTION fn::{}", name),
        detail: None,
        kind: SymbolKind::FUNCTION,
        tags: None,
        deprecated: None,
        range,
        selection_range,
        children: None,
    })
}

#[allow(deprecated)]
fn parse_define_index(
    node: &tree_sitter::Node,
    source: &str,
) -> Option<(String, DocumentSymbol)> {
    let index_name = find_name_after_keyword(node, source, &["INDEX"])?;
    let table_name = find_on_table(node, source).unwrap_or_default();
    let range = node_range(node, source);
    let selection_range = find_identifier_range(node, source).unwrap_or(range);

    Some((
        table_name,
        DocumentSymbol {
            name: format!("INDEX {}", index_name),
            detail: None,
            kind: SymbolKind::KEY,
            tags: None,
            deprecated: None,
            range,
            selection_range,
            children: None,
        },
    ))
}

#[allow(deprecated)]
fn parse_define_event(
    node: &tree_sitter::Node,
    source: &str,
) -> Option<(String, DocumentSymbol)> {
    let event_name = find_name_after_keyword(node, source, &["EVENT"])?;
    let table_name = find_on_table(node, source).unwrap_or_default();
    let range = node_range(node, source);
    let selection_range = find_identifier_range(node, source).unwrap_or(range);

    Some((
        table_name,
        DocumentSymbol {
            name: format!("EVENT {}", event_name),
            detail: None,
            kind: SymbolKind::EVENT,
            tags: None,
            deprecated: None,
            range,
            selection_range,
            children: None,
        },
    ))
}

#[allow(deprecated)]
fn parse_define_param(node: &tree_sitter::Node, source: &str) -> Option<DocumentSymbol> {
    let name = find_child_text(node, source, "variable")
        .or_else(|| find_child_text(node, source, "variable_name"))
        .or_else(|| find_name_after_keyword(node, source, &["PARAM"]))?;

    let range = node_range(node, source);
    let selection_range = find_node_range(node, source, "variable")
        .or_else(|| find_node_range(node, source, "variable_name"))
        .or_else(|| find_identifier_range(node, source))
        .unwrap_or(range);

    Some(DocumentSymbol {
        name: format!("PARAM {}", name),
        detail: None,
        kind: SymbolKind::CONSTANT,
        tags: None,
        deprecated: None,
        range,
        selection_range,
        children: None,
    })
}

#[allow(deprecated)]
fn parse_let_statement(node: &tree_sitter::Node, source: &str) -> Option<DocumentSymbol> {
    let name = find_child_text(node, source, "variable")
        .or_else(|| find_child_text(node, source, "variable_name"))?;

    let range = node_range(node, source);
    let selection_range = find_node_range(node, source, "variable")
        .or_else(|| find_node_range(node, source, "variable_name"))
        .unwrap_or(range);

    Some(DocumentSymbol {
        name: format!("LET {}", name),
        detail: None,
        kind: SymbolKind::VARIABLE,
        tags: None,
        deprecated: None,
        range,
        selection_range,
        children: None,
    })
}

// ── Helpers ──────────────────────────────────────────────────

/// Convert a tree-sitter node to an LSP Range.
fn node_range(node: &tree_sitter::Node, source: &str) -> Range {
    byte_range_to_lsp(source, node.start_byte(), node.end_byte())
}

/// Find the first identifier child and return its range.
fn find_identifier_range(node: &tree_sitter::Node, source: &str) -> Option<Range> {
    find_node_range(node, source, "identifier")
}

/// Find the first child of a given kind and return its range.
fn find_node_range(node: &tree_sitter::Node, source: &str, kind: &str) -> Option<Range> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(byte_range_to_lsp(source, child.start_byte(), child.end_byte()));
        }
    }
    None
}

/// Find the first child of a given kind and return its text.
fn find_child_text(
    node: &tree_sitter::Node,
    source: &str,
    kind: &str,
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return child
                .utf8_text(source.as_bytes())
                .ok()
                .map(|s| s.trim().to_string());
        }
    }
    None
}

/// Find the name identifier that follows one of the given keywords.
/// E.g., in `DEFINE TABLE user`, finds "user" after "TABLE".
fn find_name_after_keyword(
    node: &tree_sitter::Node,
    source: &str,
    keywords: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();

    for (i, child) in children.iter().enumerate() {
        let text = child.utf8_text(source.as_bytes()).ok()?;
        let upper = text.trim().to_uppercase();
        if keywords.iter().any(|kw| upper == *kw) {
            // The next non-keyword child is the name.
            for next in &children[i + 1..] {
                let next_text = next.utf8_text(source.as_bytes()).ok()?;
                let trimmed = next_text.trim();
                if !trimmed.is_empty()
                    && next.kind() != "keyword"
                    && !trimmed.to_uppercase().starts_with("IF")
                {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

/// Find the table name from an ON [TABLE] clause.
fn find_on_table(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "on_table_clause" {
            let mut inner = child.walk();
            for c in child.children(&mut inner) {
                if c.kind() == "identifier" {
                    return c
                        .utf8_text(source.as_bytes())
                        .ok()
                        .map(|s| s.trim().to_string());
                }
            }
        }
    }
    None
}

/// Extract detail string for a table (e.g., "SCHEMAFULL" or "TYPE RELATION").
fn extract_table_detail(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let text = node.utf8_text(source.as_bytes()).ok()?;
    let upper = text.to_uppercase();
    if upper.contains("TYPE RELATION") {
        Some("TYPE RELATION".to_string())
    } else if upper.contains("SCHEMAFULL") {
        Some("SCHEMAFULL".to_string())
    } else if upper.contains("SCHEMALESS") {
        Some("SCHEMALESS".to_string())
    } else {
        None
    }
}

/// Extract the TYPE clause from a DEFINE FIELD statement for the detail string.
fn extract_field_type(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "type_clause" {
            // Get everything after TYPE keyword.
            let mut inner = child.walk();
            for c in child.children(&mut inner) {
                if c.kind() == "type" || c.kind() != "keyword" {
                    let text = c.utf8_text(source.as_bytes()).ok()?;
                    let trimmed = text.trim();
                    if !trimmed.is_empty() && trimmed.to_uppercase() != "TYPE" {
                        return Some(trimmed.to_string());
                    }
                }
            }
        }
    }
    None
}
