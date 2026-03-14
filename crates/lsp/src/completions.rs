//! Context-aware completions for SurrealQL.
//!
//! Uses tree-sitter to understand cursor context and provides relevant
//! suggestions. Each context shows ONLY what makes sense there.

use tower_lsp::lsp_types::*;

use surrealguard_analyzer::{self as sg, Context};
use surrealguard_analyzer::context::TableKind;
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

    let tree = sg::parse(source).ok();
    let completion_ctx = tree.as_ref().and_then(|t| {
        let root = t.root_node();
        let node = root.descendant_for_byte_range(offset.saturating_sub(1), offset)?;
        Some(determine_context(&node, source, offset))
    }).unwrap_or(Ctx::General);

    let mut items = Vec::new();

    match completion_ctx {
        // ── Table targets (only tables, no variables) ────────
        Ctx::TableTarget => {
            add_non_relation_tables(ctx, &prefix, &mut items);
        }
        // ── FROM clause (tables + variables, since FROM $var is valid) ──
        Ctx::FromClause => {
            add_non_relation_tables(ctx, &prefix, &mut items);
            add_variables(ctx, &prefix, &mut items);
        }
        // ── SET clause fields (only fields of the target table) ──────
        Ctx::SetFields(table) => {
            add_fields(ctx, &table, &prefix, &mut items);
        }
        // ── WHERE / SELECT / general expression context ──────
        Ctx::Expression(table) => {
            if let Some(ref tbl) = table {
                add_fields(ctx, tbl, &prefix, &mut items);
            }
            add_variables(ctx, &prefix, &mut items);
            add_builtin_namespaces(&prefix, &mut items);
        }
        // ── After -> (only relation tables, filtered by source) ──
        Ctx::GraphEdge(source_table) => {
            add_graph_edges(ctx, source_table.as_deref(), &prefix, &mut items);
        }
        // ── After ->relation-> (only valid target tables) ──
        Ctx::GraphTarget(relation) => {
            add_graph_targets(ctx, &relation, &prefix, &mut items);
        }
        // ── After $ (variables only) ──
        Ctx::Variable => {
            add_variables(ctx, &prefix, &mut items);
        }
        // ── After namespace:: (functions in that namespace) ──
        Ctx::FunctionInNamespace(ns) => {
            add_namespace_functions(&ns, &prefix, &mut items);
        }
        // ── DEFINE FIELD ... DEFAULT/VALUE/ASSERT context ──
        Ctx::FieldExpression(table) => {
            add_fields(ctx, &table, &prefix, &mut items);
            add_variables(ctx, &prefix, &mut items);
            add_builtin_namespaces(&prefix, &mut items);
            // Add $this, $value, $before, $after for field context
        }
        // ── DEFINE TABLE PERMISSIONS / DEFINE FIELD PERMISSIONS ──
        Ctx::PermissionExpression => {
            add_variables(ctx, &prefix, &mut items);
            // Suggest FULL, NONE, WHERE
            for kw in &["FULL", "NONE", "WHERE"] {
                if prefix.is_empty() || kw.to_lowercase().starts_with(&prefix.to_lowercase()) {
                    items.push(CompletionItem {
                        label: kw.to_string(),
                        kind: Some(CompletionItemKind::KEYWORD),
                        ..CompletionItem::default()
                    });
                }
            }
        }
        // ── CONTENT clause (field names as object keys with : separator) ──
        Ctx::ContentObject(table) => {
            add_object_key_fields(ctx, &table, &prefix, &mut items);
        }
        // ── TYPE clause (type names) ──
        Ctx::TypeName => {
            add_type_names(&prefix, &mut items);
        }
        // ── General / fallback ──
        Ctx::General => {
            if prefix.starts_with('$') {
                add_variables(ctx, &prefix, &mut items);
            } else if let Some(ns) = extract_namespace_from_text(&source[..offset.min(source.len())]) {
                add_namespace_functions(&ns, &prefix, &mut items);
            } else {
                add_non_relation_tables(ctx, &prefix, &mut items);
                add_variables(ctx, &prefix, &mut items);
                add_builtin_namespaces(&prefix, &mut items);
            }
        }
    }

    items
}

// ── Context Types ────────────────────────────────────────────

#[derive(Debug)]
enum Ctx {
    /// CREATE/UPDATE/DELETE/UPSERT target — tables only, no relations
    TableTarget,
    /// FROM clause — tables + variables
    FromClause,
    /// SET clause — only fields of the target table
    SetFields(String),
    /// WHERE/SELECT expression — fields + variables + functions
    Expression(Option<String>),
    /// After -> — relation tables (optionally filtered by source table)
    GraphEdge(Option<String>),
    /// After ->relation-> — valid target tables for that relation
    GraphTarget(String),
    /// After $ — variables only
    Variable,
    /// After namespace:: — functions in that namespace
    FunctionInNamespace(String),
    /// DEFINE FIELD ... DEFAULT/VALUE/ASSERT expression
    FieldExpression(String),
    /// PERMISSIONS clause
    PermissionExpression,
    /// CONTENT { ... } — field names as object keys
    ContentObject(String),
    /// TYPE clause — type names
    TypeName,
    /// General / unknown context
    General,
}

fn determine_context(node: &tree_sitter::Node, source: &str, offset: usize) -> Ctx {
    // Text-based triggers first — these work even with parse errors
    let text_before = &source[..offset.min(source.len())];
    let text_trimmed = text_before.trim_end();

    if text_before.ends_with('$') {
        return Ctx::Variable;
    }
    if let Some(ns) = extract_namespace_from_text(text_before) {
        return Ctx::FunctionInNamespace(ns);
    }
    if text_before.ends_with("->") {
        // Find the source table for graph edge filtering
        let source_table = find_statement_table_from_text(text_trimmed);
        return Ctx::GraphEdge(source_table);
    }

    // Text-based keyword detection for incomplete parses.
    // When tree-sitter can't parse "UPDATE user SET n", we detect SET from text.
    if let Some(ctx) = detect_keyword_context(text_trimmed, source) {
        return ctx;
    }

    let mut current = *node;
    loop {
        let Some(parent) = current.parent() else {
            break;
        };

        match parent.kind() {
            "from_clause" => return Ctx::FromClause,
            "create_target" => return Ctx::TableTarget,
            // ORDER BY, GROUP BY, SPLIT, OMIT, FETCH → field names
            "order_clause" | "group_clause" | "split_clause" | "omit_clause"
            | "fetch_clause" => {
                let table = find_statement_table_from_node(&parent, source);
                return Ctx::Expression(table);
            }
            // RETURN clause
            "return_clause" => {
                let table = find_statement_table_from_node(&parent, source);
                return Ctx::Expression(table);
            }
            // Inside an object that's part of a CONTENT clause
            "object" | "object_content" | "object_property" => {
                // Check if this object is inside a content_clause
                let mut obj = parent;
                while let Some(gp) = obj.parent() {
                    if gp.kind() == "content_clause" {
                        let table = find_statement_table_from_node(&gp, source)
                            .unwrap_or_default();
                        return Ctx::ContentObject(table);
                    }
                    if gp.kind().ends_with("_statement") {
                        break;
                    }
                    obj = gp;
                }
            }
            "graph_path" | "graph_predicate" => {
                // Determine if this is the first segment (relation) or second (target)
                let prev_relation = find_preceding_relation_name(&current, source);
                if let Some(relation) = prev_relation {
                    return Ctx::GraphTarget(relation);
                }
                // First segment — suggest relation tables
                let source_table = find_statement_table_from_node(&parent, source);
                return Ctx::GraphEdge(source_table);
            }
            "set_clause" | "field_assignment" => {
                // In SET clause — are we on the field name (left of =) or value (right of =)?
                let has_equals = {
                    let mut c = parent.walk();
                    let children: Vec<_> = parent.children(&mut c).collect();
                    children.iter().any(|ch| {
                        ch.utf8_text(source.as_bytes()).ok().map(|t| t.trim()) == Some("=")
                            && ch.start_byte() < current.start_byte()
                    })
                };
                if has_equals {
                    // Right side of = — expression context
                    let table = find_statement_table_from_node(&parent, source);
                    return Ctx::Expression(table);
                } else {
                    // Left side — field names only
                    let table = find_statement_table_from_node(&parent, source)
                        .unwrap_or_default();
                    return Ctx::SetFields(table);
                }
            }
            "content_clause" => {
                // Inside CONTENT { ... } — suggest field names as keys
                let table = find_statement_table_from_node(&parent, source)
                    .unwrap_or_default();
                return Ctx::ContentObject(table);
            }
            "where_clause" => {
                let table = find_statement_table_from_node(&parent, source);
                return Ctx::Expression(table);
            }
            "select_clause" => {
                let table = find_statement_table_from_node(&parent, source);
                return Ctx::Expression(table);
            }
            "default_clause" | "value_clause" | "assert_clause" => {
                // Inside DEFINE FIELD ... DEFAULT/VALUE/ASSERT
                let table = find_define_field_table(&parent, source)
                    .unwrap_or_default();
                return Ctx::FieldExpression(table);
            }
            "permissions_for_clause" | "permissions_basic_clause" => {
                return Ctx::PermissionExpression;
            }
            "type_clause" | "type" | "parameterized_type" => {
                return Ctx::TypeName;
            }
            _ => {}
        }

        // Check previous sibling keywords
        if matches!(current.kind(), "identifier" | "ERROR" | "variable_name") {
            if let Some(prev) = current.prev_named_sibling() {
                match prev.kind() {
                    "keyword_from" => return Ctx::FromClause,
                    "keyword_into" => return Ctx::TableTarget,
                    "keyword_set" => {
                        let table = find_statement_table_from_node(&current, source)
                            .unwrap_or_default();
                        return Ctx::SetFields(table);
                    }
                    "keyword_where" => {
                        let table = find_statement_table_from_node(&current, source);
                        return Ctx::Expression(table);
                    }
                    "keyword_content" => {
                        let table = find_statement_table_from_node(&current, source)
                            .unwrap_or_default();
                        return Ctx::ContentObject(table);
                    }
                    "keyword_type" => return Ctx::TypeName,
                    "keyword_default" | "keyword_value" | "keyword_assert" => {
                        let table = find_define_field_table(&current, source)
                            .unwrap_or_default();
                        return Ctx::FieldExpression(table);
                    }
                    _ => {}
                }
            }
        }

        // DML statement targets (first identifier before any clause)
        if parent.kind().ends_with("_statement") && current.kind() == "identifier" {
            match parent.kind() {
                "create_statement" | "update_statement" | "delete_statement" | "upsert_statement" => {
                    let mut cursor = parent.walk();
                    let children: Vec<_> = parent.named_children(&mut cursor).collect();
                    for child in &children {
                        if child.kind().ends_with("_clause") || child.kind() == "set_clause" {
                            break;
                        }
                        if child.id() == current.id() {
                            return Ctx::TableTarget;
                        }
                    }
                }
                _ => {}
            }
        }

        current = parent;
    }

    Ctx::General
}

// ── Completion Providers ─────────────────────────────────────

fn add_non_relation_tables(ctx: &Context, prefix: &str, items: &mut Vec<CompletionItem>) {
    for name in ctx.table_names() {
        if !prefix.is_empty() && !name.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        let table = ctx.get_table(name);
        // Skip relation tables for non-graph contexts
        if let Some(t) = table {
            if matches!(t.kind, TableKind::Relation { .. }) {
                continue;
            }
        }
        let detail = table.map(|t| match t.schema_mode {
            sg::context::SchemaMode::Schemafull => "SCHEMAFULL".to_string(),
            sg::context::SchemaMode::Schemaless => "SCHEMALESS".to_string(),
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

fn add_fields(ctx: &Context, table: &str, prefix: &str, items: &mut Vec<CompletionItem>) {
    for field in ctx.get_fields(table) {
        if !prefix.is_empty() && !field.name.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        let detail = field.typ.as_ref().map(|t| display_kind(t));
        items.push(CompletionItem {
            label: field.name.clone(),
            kind: Some(CompletionItemKind::FIELD),
            detail,
            sort_text: Some(format!("0-{}", field.name)),
            ..CompletionItem::default()
        });
    }
}

/// Add fields as object keys (for CONTENT clause) — appends `: ` after the name
fn add_object_key_fields(ctx: &Context, table: &str, prefix: &str, items: &mut Vec<CompletionItem>) {
    for field in ctx.get_fields(table) {
        if !prefix.is_empty() && !field.name.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        let detail = field.typ.as_ref().map(|t| display_kind(t));
        items.push(CompletionItem {
            label: format!("{}: ", field.name),
            kind: Some(CompletionItemKind::FIELD),
            detail,
            filter_text: Some(field.name.clone()),
            sort_text: Some(format!("0-{}", field.name)),
            ..CompletionItem::default()
        });
    }
}

fn add_variables(ctx: &Context, prefix: &str, items: &mut Vec<CompletionItem>) {
    // User-defined variables
    for binding in ctx.scope.all_bindings() {
        let name = &binding.name;
        if !prefix.is_empty() && !name.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        let detail = if binding.typ.is_any() { None } else { Some(display_kind(&binding.typ)) };
        items.push(CompletionItem {
            label: name.clone(),
            kind: Some(CompletionItemKind::VARIABLE),
            detail,
            sort_text: Some(format!("1-{}", name)),
            ..CompletionItem::default()
        });
    }
    // Built-in variables
    for (name, description) in BUILTIN_VARIABLES {
        let var = format!("${name}");
        if !prefix.is_empty() && !var.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        items.push(CompletionItem {
            label: var,
            kind: Some(CompletionItemKind::VARIABLE),
            detail: Some(description.to_string()),
            sort_text: Some(format!("2-{}", name)),
            ..CompletionItem::default()
        });
    }
}

fn add_graph_edges(ctx: &Context, source_table: Option<&str>, prefix: &str, items: &mut Vec<CompletionItem>) {
    for name in ctx.table_names() {
        let table = match ctx.get_table(name) {
            Some(t) => t,
            None => continue,
        };
        if let TableKind::Relation { from, to } = &table.kind {
            if !prefix.is_empty() && !name.to_lowercase().starts_with(&prefix.to_lowercase()) {
                continue;
            }
            // Filter: only show relations that connect FROM the source table
            if let Some(src) = source_table {
                if let Some(from_tables) = from {
                    if !from_tables.iter().any(|t| t == src) {
                        continue;
                    }
                }
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

fn add_graph_targets(ctx: &Context, relation: &str, prefix: &str, items: &mut Vec<CompletionItem>) {
    // Show only tables that the relation connects TO
    if let Some(table) = ctx.get_table(relation) {
        if let TableKind::Relation { to, .. } = &table.kind {
            if let Some(to_tables) = to {
                for target in to_tables {
                    if !prefix.is_empty() && !target.to_lowercase().starts_with(&prefix.to_lowercase()) {
                        continue;
                    }
                    let detail = ctx.get_table(target).map(|t| match t.schema_mode {
                        sg::context::SchemaMode::Schemafull => "SCHEMAFULL".to_string(),
                        sg::context::SchemaMode::Schemaless => "SCHEMALESS".to_string(),
                    });
                    items.push(CompletionItem {
                        label: target.clone(),
                        kind: Some(CompletionItemKind::STRUCT),
                        detail,
                        sort_text: Some(format!("0-{}", target)),
                        ..CompletionItem::default()
                    });
                }
            }
        }
    }
}

fn add_namespace_functions(namespace: &str, prefix: &str, items: &mut Vec<CompletionItem>) {
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

fn add_builtin_namespaces(prefix: &str, items: &mut Vec<CompletionItem>) {
    for ns in BUILTIN_NAMESPACES {
        if !prefix.is_empty() && !ns.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        items.push(CompletionItem {
            label: format!("{}::", ns),
            kind: Some(CompletionItemKind::MODULE),
            detail: Some("function namespace".to_string()),
            sort_text: Some(format!("5-{}", ns)),
            ..CompletionItem::default()
        });
    }
}

fn add_type_names(prefix: &str, items: &mut Vec<CompletionItem>) {
    for type_name in TYPE_NAMES {
        if !prefix.is_empty() && !type_name.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        items.push(CompletionItem {
            label: type_name.to_string(),
            kind: Some(CompletionItemKind::TYPE_PARAMETER),
            sort_text: Some(format!("0-{}", type_name)),
            ..CompletionItem::default()
        });
    }
}

// ── Helpers ──────────────────────────────────────────────────

/// Text-based keyword context detection for when tree-sitter can't parse incomplete input.
/// Scans backwards from cursor to find the last significant keyword.
fn detect_keyword_context(text: &str, _full_source: &str) -> Option<Ctx> {
    // Only look at the current statement (after the last semicolon)
    let current_stmt = text.rfind(';').map(|i| &text[i + 1..]).unwrap_or(text);
    let upper = current_stmt.to_uppercase();
    let words: Vec<&str> = upper.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }

    // Find the last keyword and determine context
    for i in (0..words.len()).rev() {
        match words[i] {
            "SET" => {
                // Find the table name: look for CREATE/UPDATE/UPSERT before SET
                let table = find_dml_table_from_words(&words[..i]);
                return Some(Ctx::SetFields(table.unwrap_or_default()));
            }
            "CONTENT" => {
                let table = find_dml_table_from_words(&words[..i]);
                return Some(Ctx::ContentObject(table.unwrap_or_default()));
            }
            "WHERE" => {
                let table = find_dml_table_from_words(&words[..i]);
                return Some(Ctx::Expression(table));
            }
            "FROM" => return Some(Ctx::FromClause),
            "INTO" => return Some(Ctx::TableTarget),
            "CREATE" | "UPDATE" | "DELETE" | "UPSERT" => {
                // If this is the last word, we need a table name
                if i == words.len() - 1 {
                    return Some(Ctx::TableTarget);
                }
            }
            "ORDER" | "GROUP" | "SPLIT" | "OMIT" | "FETCH" => {
                // After ORDER BY, GROUP BY, etc. — field names
                let table = find_dml_table_from_words(&words[..i]);
                return Some(Ctx::Expression(table));
            }
            "BY" => {
                // Check if preceded by ORDER or GROUP
                if i > 0 && matches!(words[i - 1], "ORDER" | "GROUP") {
                    let table = find_dml_table_from_words(&words[..i - 1]);
                    return Some(Ctx::Expression(table));
                }
            }
            "RETURN" => {
                // RETURN clause — suggest BEFORE, AFTER, DIFF, NONE, or fields
                return Some(Ctx::General); // TODO: dedicated ReturnClause context
            }
            "TYPE" => {
                return Some(Ctx::TypeName);
            }
            "DEFAULT" | "VALUE" | "ASSERT" => {
                // DEFINE FIELD context — find the table from ON clause
                let table = find_on_table_from_words(&words);
                return Some(Ctx::FieldExpression(table.unwrap_or_default()));
            }
            "ON" => {
                // After ON — suggest table names
                return Some(Ctx::TableTarget);
            }
            "INDEX" => {
                // After WITH INDEX — suggest index names (not implemented yet)
                if i > 0 && words[i - 1] == "WITH" {
                    return None; // Let general fallback handle
                }
            }
            _ => {}
        }
    }

    None
}

/// Extract table name from DML word sequence like ["CREATE", "user"] or ["UPDATE", "user", "SET"]
fn find_dml_table_from_words(words: &[&str]) -> Option<String> {
    for i in 0..words.len() {
        match words[i] {
            "CREATE" | "UPDATE" | "DELETE" | "UPSERT" | "INSERT" => {
                // Next word is the table (skip INTO for INSERT)
                let next = if words[i] == "INSERT" && i + 1 < words.len() && words[i + 1] == "INTO" {
                    i + 2
                } else {
                    i + 1
                };
                if next < words.len() {
                    return Some(words[next].to_lowercase());
                }
            }
            "FROM" => {
                if i + 1 < words.len() {
                    return Some(words[i + 1].to_lowercase());
                }
            }
            _ => {}
        }
    }
    None
}

/// Extract table from the NEAREST ON clause, searching backwards from cursor.
/// This ensures we find the ON clause of the current DEFINE FIELD, not a previous one.
fn find_on_table_from_words(words: &[&str]) -> Option<String> {
    // Search backwards to find the nearest ON clause
    for i in (0..words.len()).rev() {
        if words[i] == "ON" {
            let next = if i + 1 < words.len() && words[i + 1] == "TABLE" {
                i + 2
            } else {
                i + 1
            };
            if next < words.len() {
                return Some(words[next].to_lowercase());
            }
        }
    }
    None
}

/// Extract table name from text for graph edge context
fn find_statement_table_from_text(text: &str) -> Option<String> {
    let upper = text.to_uppercase();
    let words: Vec<&str> = upper.split_whitespace().collect();
    find_dml_table_from_words(&words)
}

fn extract_prefix(source: &str, offset: usize) -> String {
    let before = &source[..offset.min(source.len())];
    let start = before.rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '$')
        .map(|i| i + 1)
        .unwrap_or(0);
    before[start..].to_string()
}

fn extract_namespace_from_text(text: &str) -> Option<String> {
    if !text.ends_with("::") {
        return None;
    }
    let before = &text[..text.len() - 2];
    let start = before.rfind(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    let ns = &before[start..];
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

fn find_define_field_table(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let mut current = *node;
    while let Some(parent) = current.parent() {
        if parent.kind() == "define_field_statement" {
            // Find ON TABLE clause
            let mut cursor = parent.walk();
            for child in parent.named_children(&mut cursor) {
                if child.kind() == "on_table_clause" {
                    let mut inner = child.walk();
                    for c in child.named_children(&mut inner) {
                        if c.kind() == "identifier" {
                            return c.utf8_text(source.as_bytes()).ok().map(|s| s.trim().to_string());
                        }
                    }
                }
            }
        }
        current = parent;
    }
    None
}

/// Find the relation name in a preceding graph segment.
fn find_preceding_relation_name(node: &tree_sitter::Node, source: &str) -> Option<String> {
    // Walk up to the path node
    let mut current = *node;
    while let Some(parent) = current.parent() {
        if parent.kind() == "path" {
            let mut cursor = parent.walk();
            let children: Vec<_> = parent.named_children(&mut cursor).collect();
            let our_idx = children.iter().position(|c| {
                c.start_byte() <= node.start_byte() && c.end_byte() >= node.end_byte()
            })?;
            if our_idx > 0 {
                return first_identifier_text(&children[our_idx - 1], source);
            }
            return None;
        }
        current = parent;
    }
    None
}

fn first_identifier_text(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let mut stack = vec![*node];
    while let Some(current) = stack.pop() {
        if current.kind() == "identifier" {
            return current.utf8_text(source.as_bytes()).ok().map(|s| s.trim().to_string());
        }
        let mut cursor = current.walk();
        for child in current.named_children(&mut cursor) {
            stack.push(child);
        }
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

static TYPE_NAMES: &[&str] = &[
    "any", "null", "bool", "int", "float", "decimal", "number",
    "string", "bytes", "duration", "datetime", "uuid", "object",
    "array", "set", "record", "geometry", "option", "range",
];
