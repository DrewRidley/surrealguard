//! Context-aware completions for SurrealQL.
//!
//! Uses tree-sitter to understand cursor context and provides relevant
//! suggestions: table names after FROM, field names in WHERE/SET,
//! function names after ::, variables after $.

use tower_lsp::lsp_types::*;

use surrealguard_analyzer::{self as sg, Context};
use surrealguard_analyzer::types::{display_kind, KindExt};

use crate::text::position_to_offset;
use crate::hover::BUILTIN_FUNCTIONS;

/// Resolve completions for a document at a given position.
pub fn resolve(
    source: &str,
    position: Position,
    ctx: &Context,
) -> Vec<CompletionItem> {
    let offset = position_to_offset(source, position);
    let prefix = extract_prefix(source, offset);
    let trigger = detect_trigger(source, offset);

    let tree = sg::parse(source).ok();
    let node_context = tree.as_ref().and_then(|t| {
        let root = t.root_node();
        let node = root.descendant_for_byte_range(offset.saturating_sub(1), offset)?;
        Some(determine_context(&node, source))
    }).unwrap_or(CompletionContext::Unknown);

    let mut items = Vec::new();

    match node_context {
        CompletionContext::AfterFrom | CompletionContext::AfterInto
        | CompletionContext::AfterUpdate | CompletionContext::AfterDelete => {
            add_table_completions(ctx, &prefix, &mut items);
        }
        CompletionContext::AfterSet | CompletionContext::InWhere
        | CompletionContext::InSelect => {
            // Find the statement's target table for field completions
            if let Some(ref tree) = tree {
                let root = tree.root_node();
                if let Some(node) = root.descendant_for_byte_range(offset.saturating_sub(1), offset) {
                    let table = find_statement_table_from_node(&node, source);
                    if let Some(tbl) = table {
                        add_field_completions(ctx, &tbl, &prefix, &mut items);
                    }
                }
            }
            // Also add variables and tables as fallback
            add_variable_completions(ctx, &prefix, &mut items);
            add_table_completions(ctx, &prefix, &mut items);
        }
        CompletionContext::AfterArrow => {
            // After -> in graph traversal, suggest relation tables
            add_relation_table_completions(ctx, &prefix, &mut items);
        }
        CompletionContext::AfterDollar => {
            add_variable_completions(ctx, &prefix, &mut items);
        }
        CompletionContext::AfterDoubleColon(namespace) => {
            add_namespace_function_completions(&namespace, &prefix, &mut items);
        }
        CompletionContext::Unknown => {
            // General context — suggest based on trigger character
            match trigger {
                Some('$') => add_variable_completions(ctx, &prefix, &mut items),
                Some(':') => {
                    // Could be :: for function namespace
                    let ns = extract_namespace(source, offset);
                    if let Some(namespace) = ns {
                        add_namespace_function_completions(&namespace, &prefix, &mut items);
                    }
                }
                _ => {
                    // Suggest keywords, tables, functions
                    add_keyword_completions(&prefix, &mut items);
                    add_table_completions(ctx, &prefix, &mut items);
                    add_variable_completions(ctx, &prefix, &mut items);
                    add_builtin_namespace_completions(&prefix, &mut items);
                }
            }
        }
    }

    items
}

// ── Context Detection ────────────────────────────────────────

#[derive(Debug)]
enum CompletionContext {
    AfterFrom,
    AfterInto,
    AfterUpdate,
    AfterDelete,
    AfterSet,
    InWhere,
    InSelect,
    AfterArrow,
    AfterDollar,
    AfterDoubleColon(String),
    Unknown,
}

fn determine_context(node: &tree_sitter::Node, source: &str) -> CompletionContext {
    let mut current = *node;
    loop {
        let Some(parent) = current.parent() else {
            break;
        };

        match parent.kind() {
            "from_clause" => return CompletionContext::AfterFrom,
            "create_target" => return CompletionContext::AfterFrom,
            "set_clause" | "field_assignment" => return CompletionContext::AfterSet,
            "where_clause" => return CompletionContext::InWhere,
            "select_clause" => return CompletionContext::InSelect,
            "graph_path" | "graph_predicate" => return CompletionContext::AfterArrow,
            _ => {}
        }

        // Check if we're right after a keyword
        if current.kind() == "identifier" || current.kind() == "ERROR" {
            let prev = current.prev_named_sibling();
            if let Some(prev_node) = prev {
                match prev_node.kind() {
                    "keyword_from" => return CompletionContext::AfterFrom,
                    "keyword_into" => return CompletionContext::AfterInto,
                    "keyword_update" => return CompletionContext::AfterUpdate,
                    "keyword_delete" => return CompletionContext::AfterDelete,
                    "keyword_set" => return CompletionContext::AfterSet,
                    "keyword_where" => return CompletionContext::InWhere,
                    _ => {}
                }
            }
        }

        // Check for DML statement targets
        if parent.kind().ends_with("_statement") {
            match parent.kind() {
                "update_statement" | "delete_statement" | "upsert_statement" => {
                    // If we're the first identifier in the statement, it's a table target
                    let mut cursor = parent.walk();
                    let children: Vec<_> = parent.named_children(&mut cursor).collect();
                    for child in &children {
                        if child.kind().ends_with("_clause") {
                            break;
                        }
                        if child.id() == current.id() {
                            return CompletionContext::AfterUpdate;
                        }
                    }
                }
                _ => {}
            }
        }

        current = parent;
    }

    // Check text-based triggers
    let text_before = &source[..node.start_byte().min(source.len())];
    if text_before.ends_with("$") {
        return CompletionContext::AfterDollar;
    }
    if text_before.ends_with("->") {
        return CompletionContext::AfterArrow;
    }
    if let Some(ns) = extract_namespace_from_text(text_before) {
        return CompletionContext::AfterDoubleColon(ns);
    }

    CompletionContext::Unknown
}

// ── Completion Providers ─────────────────────────────────────

fn add_table_completions(ctx: &Context, prefix: &str, items: &mut Vec<CompletionItem>) {
    for name in ctx.table_names() {
        if !prefix.is_empty() && !name.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        let table = ctx.get_table(name);
        let detail = table.map(|t| {
            match &t.kind {
                surrealguard_analyzer::context::TableKind::Relation { from, to } => {
                    let from_str = from.as_ref().map(|f| f.join("|")).unwrap_or_default();
                    let to_str = to.as_ref().map(|t| t.join("|")).unwrap_or_default();
                    format!("RELATION {} → {}", from_str, to_str)
                }
                _ => match t.schema_mode {
                    surrealguard_analyzer::context::SchemaMode::Schemafull => "SCHEMAFULL".to_string(),
                    surrealguard_analyzer::context::SchemaMode::Schemaless => "SCHEMALESS".to_string(),
                }
            }
        });

        items.push(CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::STRUCT),
            detail,
            sort_text: Some(format!("0-{}", name)),
            ..CompletionItem::default()
        });
    }
}

fn add_field_completions(ctx: &Context, table: &str, prefix: &str, items: &mut Vec<CompletionItem>) {
    for field in ctx.get_fields(table) {
        if !prefix.is_empty() && !field.name.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        let detail = field.typ.as_ref().map(|t| display_kind(t));
        items.push(CompletionItem {
            label: field.name.clone(),
            kind: Some(CompletionItemKind::FIELD),
            detail,
            sort_text: Some(format!("1-{}", field.name)),
            ..CompletionItem::default()
        });
    }
}

fn add_variable_completions(ctx: &Context, prefix: &str, items: &mut Vec<CompletionItem>) {
    for binding in ctx.scope.all_bindings() {
        let name = &binding.name;
        if !prefix.is_empty() && !name.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        let detail = if binding.typ.is_any() {
            None
        } else {
            Some(display_kind(&binding.typ))
        };
        items.push(CompletionItem {
            label: name.clone(),
            kind: Some(CompletionItemKind::VARIABLE),
            detail,
            sort_text: Some(format!("2-{}", name)),
            ..CompletionItem::default()
        });
    }

    // Always suggest built-in variables
    for (name, description) in BUILTIN_VARIABLES {
        let var = format!("${name}");
        if !prefix.is_empty() && !var.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        items.push(CompletionItem {
            label: var,
            kind: Some(CompletionItemKind::VARIABLE),
            detail: Some(description.to_string()),
            sort_text: Some(format!("3-{}", name)),
            ..CompletionItem::default()
        });
    }
}

fn add_relation_table_completions(ctx: &Context, prefix: &str, items: &mut Vec<CompletionItem>) {
    for name in ctx.table_names() {
        let table = match ctx.get_table(name) {
            Some(t) => t,
            None => continue,
        };
        if let surrealguard_analyzer::context::TableKind::Relation { from, to } = &table.kind {
            if !prefix.is_empty() && !name.to_lowercase().starts_with(&prefix.to_lowercase()) {
                continue;
            }
            let from_str = from.as_ref().map(|f| f.join("|")).unwrap_or_default();
            let to_str = to.as_ref().map(|t| t.join("|")).unwrap_or_default();
            items.push(CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::STRUCT),
                detail: Some(format!("{} → {}", from_str, to_str)),
                sort_text: Some(format!("0-{}", name)),
                ..CompletionItem::default()
            });
        }
    }
}

fn add_namespace_function_completions(namespace: &str, prefix: &str, items: &mut Vec<CompletionItem>) {
    let ns_prefix = format!("{}::", namespace);
    for func in BUILTIN_FUNCTIONS {
        if !func.name.starts_with(&ns_prefix) {
            continue;
        }
        let short_name = &func.name[ns_prefix.len()..];
        if !prefix.is_empty() && !short_name.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        items.push(CompletionItem {
            label: short_name.to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(func.signature.to_string()),
            documentation: Some(Documentation::String(func.summary.to_string())),
            insert_text: Some(format!("{}($0)", short_name)),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            sort_text: Some(format!("0-{}", short_name)),
            ..CompletionItem::default()
        });
    }
}

fn add_builtin_namespace_completions(prefix: &str, items: &mut Vec<CompletionItem>) {
    for ns in BUILTIN_NAMESPACES {
        if !prefix.is_empty() && !ns.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        items.push(CompletionItem {
            label: format!("{}::", ns),
            kind: Some(CompletionItemKind::MODULE),
            detail: Some("function namespace".to_string()),
            sort_text: Some(format!("4-{}", ns)),
            ..CompletionItem::default()
        });
    }
}

fn add_keyword_completions(prefix: &str, items: &mut Vec<CompletionItem>) {
    for kw in KEYWORDS {
        if !prefix.is_empty() && !kw.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        items.push(CompletionItem {
            label: kw.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            sort_text: Some(format!("9-{}", kw)),
            ..CompletionItem::default()
        });
    }
}

// ── Helpers ──────────────────────────────────────────────────

fn extract_prefix(source: &str, offset: usize) -> String {
    let before = &source[..offset.min(source.len())];
    let start = before.rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '$')
        .map(|i| i + 1)
        .unwrap_or(0);
    before[start..].to_string()
}

fn detect_trigger(source: &str, offset: usize) -> Option<char> {
    if offset == 0 { return None; }
    source[..offset].chars().last()
}

fn extract_namespace(source: &str, offset: usize) -> Option<String> {
    extract_namespace_from_text(&source[..offset.min(source.len())])
}

fn extract_namespace_from_text(text: &str) -> Option<String> {
    if !text.ends_with("::") {
        return None;
    }
    let before_colons = &text[..text.len() - 2];
    let start = before_colons.rfind(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    let ns = &before_colons[start..];
    if ns.is_empty() { None } else { Some(ns.to_string()) }
}

fn find_statement_table_from_node(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let mut current = *node;
    while let Some(parent) = current.parent() {
        if parent.kind().ends_with("_statement") {
            return crate::hover::extract_table_from_statement_pub(&parent, source);
        }
        current = parent;
    }
    None
}

// ── Constants ────────────────────────────────────────────────

static BUILTIN_NAMESPACES: &[&str] = &[
    "array", "bytes", "crypto", "duration", "encoding", "geo", "http",
    "math", "meta", "object", "parse", "rand", "record", "search",
    "session", "string", "time", "type", "vector",
];

static BUILTIN_VARIABLES: &[(&str, &str)] = &[
    ("auth", "The authenticated user's data"),
    ("token", "The authentication token"),
    ("session", "The current session data"),
    ("this", "The current record"),
    ("parent", "The parent record"),
    ("value", "The field value (in DEFINE FIELD)"),
    ("input", "The input value"),
    ("before", "The record before modification"),
    ("after", "The record after modification"),
    ("event", "The event type (CREATE, UPDATE, DELETE)"),
];

static KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "AND", "OR", "NOT", "ORDER", "BY",
    "LIMIT", "START", "FETCH", "GROUP", "SPLIT", "OMIT", "TIMEOUT",
    "PARALLEL", "EXPLAIN",
    "CREATE", "SET", "CONTENT", "RETURN",
    "UPDATE", "MERGE", "PATCH",
    "DELETE",
    "INSERT", "INTO", "VALUES", "ON DUPLICATE KEY UPDATE",
    "UPSERT",
    "RELATE",
    "DEFINE", "TABLE", "FIELD", "INDEX", "FUNCTION", "EVENT", "PARAM",
    "SCHEMAFULL", "SCHEMALESS", "TYPE", "DEFAULT", "ASSERT", "READONLY",
    "OVERWRITE", "IF NOT EXISTS",
    "REMOVE",
    "LET", "IF", "ELSE", "END", "FOR", "IN",
    "BEGIN", "COMMIT", "CANCEL", "TRANSACTION",
    "RETURN", "THROW", "BREAK", "CONTINUE",
    "LIVE", "KILL", "SHOW", "CHANGES", "SINCE",
    "USE", "NS", "DB", "INFO",
];
