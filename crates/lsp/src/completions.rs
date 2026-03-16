//! Context-aware completions for SurrealQL.
//!
//! Uses tree-sitter's partial parse tree (including ERROR nodes) to determine
//! completion context. Even with incomplete input, the valid parts of the AST
//! and the contents of ERROR nodes provide enough information.

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

    let tree = match sg::parse(source) {
        Ok(t) => t,
        Err(_) => return general_completions(ctx, &prefix),
    };
    let root = tree.root_node();
    let node = match root.descendant_for_byte_range(offset.saturating_sub(1), offset) {
        Some(n) => n,
        None => return general_completions(ctx, &prefix),
    };

    let completion_ctx = determine_context(&node, source, offset);
    let mut items = Vec::new();
    apply_context(completion_ctx, ctx, &prefix, source, &node, &mut items);
    items
}

// ── Context Types ────────────────────────────────────────────

#[derive(Debug)]
enum Ctx {
    /// Tables only (CREATE/UPDATE/DELETE target, ON TABLE, etc.)
    TableTarget,
    /// FROM clause — tables + variables
    FromClause,
    /// SET clause field names
    SetFields(String),
    /// Expression context (WHERE, value position) — fields + vars + functions
    Expression(Option<String>),
    /// After -> — relation tables
    GraphEdge(Option<String>),
    /// After ->relation-> — valid target tables
    GraphTarget(String),
    /// Variables only (after $)
    Variable,
    /// Functions in namespace (after ::)
    FunctionInNamespace(String),
    /// CONTENT object keys — field names with : separator
    ContentObject(String),
    /// Type names (after TYPE keyword)
    TypeName,
    /// RETURN clause — BEFORE, AFTER, DIFF, NONE, fields
    ReturnClause(Option<String>),
    /// PERMISSIONS — FULL, NONE, WHERE, FOR
    PermissionClause,
    /// Inside a block — statements + expressions
    BlockStatement,
    /// General fallback
    General,
}

fn determine_context(node: &tree_sitter::Node, source: &str, offset: usize) -> Ctx {
    // Quick text checks for triggers that work regardless of parse state
    let text_before = &source[..offset.min(source.len())];
    if text_before.ends_with('$') {
        return Ctx::Variable;
    }
    if text_before.ends_with("->") {
        let table = find_from_table_in_text(text_before);
        return Ctx::GraphEdge(table);
    }
    if let Some(ns) = extract_namespace_from_text(text_before) {
        return Ctx::FunctionInNamespace(ns);
    }

    // Strategy: walk up from the cursor node through valid AST AND error nodes.
    // At each level, check if we can determine context from:
    // 1. The node's kind (if it's a valid clause)
    // 2. The node's parent's kind
    // 3. Keywords found inside ERROR nodes
    // 4. Preceding siblings of ERROR nodes

    let mut current = *node;
    let mut depth = 0;

    loop {
        if depth > 20 { break; }
        depth += 1;

        // Check if current node or its parent tells us the context
        if let Some(ctx) = check_node_context(&current, source) {
            return ctx;
        }

        // If we're in an ERROR node, examine its contents and siblings
        if current.kind() == "ERROR" {
            if let Some(ctx) = analyze_error_node(&current, source) {
                return ctx;
            }
        }

        let Some(parent) = current.parent() else {
            break;
        };

        // Check the parent's kind for clause context
        if let Some(ctx) = check_parent_context(&parent, &current, source) {
            return ctx;
        }

        current = parent;
    }

    Ctx::General
}

/// Check if a node's kind directly tells us the completion context.
fn check_node_context(node: &tree_sitter::Node, source: &str) -> Option<Ctx> {
    match node.kind() {
        "from_clause" => Some(Ctx::FromClause),
        "create_target" => Some(Ctx::TableTarget),
        "set_clause" => {
            let table = find_table_from_ancestor(node, source);
            Some(Ctx::SetFields(table.unwrap_or_default()))
        }
        "where_clause" => {
            let table = find_table_from_ancestor(node, source);
            Some(Ctx::Expression(table))
        }
        "select_clause" => {
            let table = find_table_from_ancestor(node, source);
            Some(Ctx::Expression(table))
        }
        "content_clause" => {
            let table = find_table_from_ancestor(node, source);
            Some(Ctx::ContentObject(table.unwrap_or_default()))
        }
        "return_clause" => {
            let table = find_table_from_ancestor(node, source);
            Some(Ctx::ReturnClause(table))
        }
        "order_clause" | "group_clause" | "split_clause" | "omit_clause" | "fetch_clause" => {
            let table = find_table_from_ancestor(node, source);
            Some(Ctx::Expression(table))
        }
        "graph_path" | "graph_predicate" => {
            let table = find_table_from_ancestor(node, source);
            Some(Ctx::GraphEdge(table))
        }
        "type_clause" | "type" | "parameterized_type" | "type_name" => {
            Some(Ctx::TypeName)
        }
        "permissions_for_clause" | "permissions_basic_clause" => {
            Some(Ctx::PermissionClause)
        }
        "block" | "block_expression" => {
            Some(Ctx::BlockStatement)
        }
        "default_clause" | "assert_clause" => {
            let table = find_define_field_table_from_ancestor(node, source);
            Some(Ctx::Expression(table))
        }
        _ => None,
    }
}

/// Check parent-child relationship for context.
fn check_parent_context(parent: &tree_sitter::Node, current: &tree_sitter::Node, source: &str) -> Option<Ctx> {
    match parent.kind() {
        "from_clause" => Some(Ctx::FromClause),
        "create_target" => Some(Ctx::TableTarget),
        "set_clause" | "field_assignment" => {
            // Left of = → field names. Right of = → expression.
            let is_value_side = has_preceding_equals(parent, current, source);
            if is_value_side {
                let table = find_table_from_ancestor(parent, source);
                Some(Ctx::Expression(table))
            } else {
                let table = find_table_from_ancestor(parent, source);
                Some(Ctx::SetFields(table.unwrap_or_default()))
            }
        }
        "where_clause" | "select_clause" | "order_clause" | "group_clause"
        | "split_clause" | "omit_clause" | "fetch_clause" => {
            let table = find_table_from_ancestor(parent, source);
            Some(Ctx::Expression(table))
        }
        "content_clause" => {
            let table = find_table_from_ancestor(parent, source);
            Some(Ctx::ContentObject(table.unwrap_or_default()))
        }
        "object" | "object_content" | "object_property" => {
            // Check if this object is inside a content_clause
            if is_inside_content_clause(parent) {
                let table = find_table_from_ancestor(parent, source);
                Some(Ctx::ContentObject(table.unwrap_or_default()))
            } else {
                None
            }
        }
        "graph_path" | "graph_predicate" => {
            // Check if there's a preceding relation for target suggestions
            if let Some(relation) = find_preceding_relation_in_path(current, source) {
                Some(Ctx::GraphTarget(relation))
            } else {
                let table = find_table_from_ancestor(parent, source);
                Some(Ctx::GraphEdge(table))
            }
        }
        "return_clause" => {
            let table = find_table_from_ancestor(parent, source);
            Some(Ctx::ReturnClause(table))
        }
        "type_clause" | "type" | "parameterized_type" => Some(Ctx::TypeName),
        "permissions_for_clause" | "permissions_basic_clause" => Some(Ctx::PermissionClause),
        "block" | "block_expression" => Some(Ctx::BlockStatement),
        "on_table_clause" => Some(Ctx::TableTarget),
        // DML statement targets (first identifier before clauses)
        "update_statement" | "delete_statement" | "upsert_statement" | "create_statement"
            if current.kind() == "identifier" =>
        {
            if is_statement_target(parent, current) {
                Some(Ctx::TableTarget)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Analyze an ERROR node to determine context from its contents and siblings.
fn analyze_error_node(error: &tree_sitter::Node, source: &str) -> Option<Ctx> {
    // Collect keywords inside the ERROR node
    let mut keywords = Vec::new();
    let mut cursor = error.walk();
    let children: Vec<_> = error.children(&mut cursor).collect();
    for child in &children {
        if child.kind().starts_with("keyword_") {
            keywords.push(child.kind().to_string());
        }
    }

    // Find the preceding valid sibling (the statement before this error)
    let prev_sibling = error.prev_named_sibling();
    let prev_kind = prev_sibling.as_ref().map(|s| s.kind());

    // Determine table from the preceding statement
    let table = prev_sibling.as_ref()
        .and_then(|s| extract_table_from_statement(s, source));

    // Match on keywords found in the ERROR
    for kw in &keywords {
        match kw.as_str() {
            "keyword_set" => return Some(Ctx::SetFields(table.clone().unwrap_or_default())),
            "keyword_content" => return Some(Ctx::ContentObject(table.clone().unwrap_or_default())),
            "keyword_where" => return Some(Ctx::Expression(table.clone())),
            "keyword_from" => return Some(Ctx::FromClause),
            "keyword_into" => return Some(Ctx::TableTarget),
            "keyword_return" => return Some(Ctx::ReturnClause(table.clone())),
            "keyword_create" | "keyword_update" | "keyword_delete" | "keyword_upsert" => {
                // If it's the only keyword with no identifier after it → table target
                let has_ident = children.iter().any(|c| c.kind() == "identifier");
                if !has_ident {
                    return Some(Ctx::TableTarget);
                }
            }
            "keyword_order" | "keyword_group" | "keyword_split" | "keyword_omit" | "keyword_fetch" => {
                return Some(Ctx::Expression(table.clone()));
            }
            "keyword_type" => return Some(Ctx::TypeName),
            "keyword_on" => return Some(Ctx::TableTarget),
            "keyword_default" | "keyword_value" | "keyword_assert" => {
                let def_table = find_define_field_table_in_children(&children, source);
                return Some(Ctx::Expression(def_table.or(table.clone())));
            }
            "keyword_permissions" => return Some(Ctx::PermissionClause),
            _ => {}
        }
    }

    // If the preceding sibling is a DML statement and ERROR has SET/WHERE/etc
    if let Some(ref pk) = prev_kind {
        if pk.ends_with("_statement") && !keywords.is_empty() {
            // Already handled above, but for keywords not yet matched:
            return Some(Ctx::Expression(table));
        }
    }

    None
}

// ── Apply Context ────────────────────────────────────────────

fn apply_context(
    ctx_type: Ctx,
    ctx: &Context,
    prefix: &str,
    _source: &str,
    _node: &tree_sitter::Node,
    items: &mut Vec<CompletionItem>,
) {
    match ctx_type {
        Ctx::TableTarget => {
            add_non_relation_tables(ctx, prefix, items);
        }
        Ctx::FromClause => {
            add_non_relation_tables(ctx, prefix, items);
            add_variables(ctx, prefix, items);
        }
        Ctx::SetFields(table) => {
            add_fields(ctx, &table, prefix, items);
        }
        Ctx::Expression(table) => {
            if let Some(ref tbl) = table {
                add_fields(ctx, tbl, prefix, items);
            }
            add_variables(ctx, prefix, items);
            add_builtin_namespaces(prefix, items);
        }
        Ctx::GraphEdge(source_table) => {
            add_graph_edges(ctx, source_table.as_deref(), prefix, items);
        }
        Ctx::GraphTarget(relation) => {
            add_graph_targets(ctx, &relation, prefix, items);
        }
        Ctx::Variable => {
            add_variables(ctx, prefix, items);
        }
        Ctx::FunctionInNamespace(ns) => {
            add_namespace_functions(&ns, prefix, items);
        }
        Ctx::ContentObject(table) => {
            add_object_key_fields(ctx, &table, prefix, items);
        }
        Ctx::TypeName => {
            add_type_names(prefix, items);
        }
        Ctx::ReturnClause(table) => {
            // RETURN BEFORE/AFTER/DIFF/NONE + fields
            for kw in &["BEFORE", "AFTER", "DIFF", "NONE"] {
                if prefix.is_empty() || kw.to_lowercase().starts_with(&prefix.to_lowercase()) {
                    items.push(CompletionItem {
                        label: kw.to_string(),
                        kind: Some(CompletionItemKind::KEYWORD),
                        ..CompletionItem::default()
                    });
                }
            }
            if let Some(ref tbl) = table {
                add_fields(ctx, tbl, prefix, items);
            }
        }
        Ctx::PermissionClause => {
            for kw in &["FULL", "NONE", "FOR", "WHERE"] {
                if prefix.is_empty() || kw.to_lowercase().starts_with(&prefix.to_lowercase()) {
                    items.push(CompletionItem {
                        label: kw.to_string(),
                        kind: Some(CompletionItemKind::KEYWORD),
                        ..CompletionItem::default()
                    });
                }
            }
        }
        Ctx::BlockStatement => {
            // Inside a block: suggest statements + variables + functions
            add_statement_keywords(prefix, items);
            add_variables(ctx, prefix, items);
            add_builtin_namespaces(prefix, items);
        }
        Ctx::General => {
            if prefix.starts_with('$') {
                add_variables(ctx, prefix, items);
            } else {
                add_non_relation_tables(ctx, prefix, items);
                add_variables(ctx, prefix, items);
                add_builtin_namespaces(prefix, items);
            }
        }
    }
}

fn general_completions(ctx: &Context, prefix: &str) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    add_non_relation_tables(ctx, prefix, &mut items);
    add_variables(ctx, prefix, &mut items);
    add_builtin_namespaces(prefix, &mut items);
    items
}

// ── Completion Providers ─────────────────────────────────────

fn add_non_relation_tables(ctx: &Context, prefix: &str, items: &mut Vec<CompletionItem>) {
    for name in ctx.table_names() {
        if !prefix.is_empty() && !name.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        let table = ctx.get_table(name);
        if let Some(t) = table {
            if matches!(t.kind, TableKind::Relation { .. }) { continue; }
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
        items.push(CompletionItem {
            label: field.name.clone(),
            kind: Some(CompletionItemKind::FIELD),
            detail: field.typ.as_ref().map(|t| display_kind(t)),
            sort_text: Some(format!("0-{}", field.name)),
            ..CompletionItem::default()
        });
    }
}

fn add_object_key_fields(ctx: &Context, table: &str, prefix: &str, items: &mut Vec<CompletionItem>) {
    for field in ctx.get_fields(table) {
        if !prefix.is_empty() && !field.name.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        items.push(CompletionItem {
            label: format!("{}: ", field.name),
            kind: Some(CompletionItemKind::FIELD),
            detail: field.typ.as_ref().map(|t| display_kind(t)),
            filter_text: Some(field.name.clone()),
            sort_text: Some(format!("0-{}", field.name)),
            ..CompletionItem::default()
        });
    }
}

fn add_variables(ctx: &Context, prefix: &str, items: &mut Vec<CompletionItem>) {
    for binding in ctx.scope.all_bindings() {
        if !prefix.is_empty() && !binding.name.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        items.push(CompletionItem {
            label: binding.name.clone(),
            kind: Some(CompletionItemKind::VARIABLE),
            detail: if binding.typ.is_any() { None } else { Some(display_kind(&binding.typ)) },
            sort_text: Some(format!("1-{}", binding.name)),
            ..CompletionItem::default()
        });
    }
    for (name, desc) in BUILTIN_VARIABLES {
        let var = format!("${name}");
        if !prefix.is_empty() && !var.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        items.push(CompletionItem {
            label: var,
            kind: Some(CompletionItemKind::VARIABLE),
            detail: Some(desc.to_string()),
            sort_text: Some(format!("2-{}", name)),
            ..CompletionItem::default()
        });
    }
}

fn add_graph_edges(ctx: &Context, source_table: Option<&str>, prefix: &str, items: &mut Vec<CompletionItem>) {
    for name in ctx.table_names() {
        let table = match ctx.get_table(name) { Some(t) => t, None => continue };
        if let TableKind::Relation { from, to } = &table.kind {
            if !prefix.is_empty() && !name.to_lowercase().starts_with(&prefix.to_lowercase()) {
                continue;
            }
            if let Some(src) = source_table {
                if let Some(from_tables) = from {
                    if !from_tables.iter().any(|t| t == src) { continue; }
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
    if let Some(table) = ctx.get_table(relation) {
        if let TableKind::Relation { to, .. } = &table.kind {
            if let Some(to_tables) = to {
                for target in to_tables {
                    if !prefix.is_empty() && !target.to_lowercase().starts_with(&prefix.to_lowercase()) {
                        continue;
                    }
                    items.push(CompletionItem {
                        label: target.clone(),
                        kind: Some(CompletionItemKind::STRUCT),
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
        if !func.name.starts_with(&ns_prefix) { continue; }
        let short = &func.name[ns_prefix.len()..];
        if !prefix.is_empty() && !short.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        items.push(CompletionItem {
            label: short.to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(func.signature.to_string()),
            documentation: Some(Documentation::String(func.summary.to_string())),
            insert_text: Some(format!("{}($0)", short)),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            sort_text: Some(format!("0-{}", short)),
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
    for tn in TYPE_NAMES {
        if !prefix.is_empty() && !tn.to_lowercase().starts_with(&prefix.to_lowercase()) {
            continue;
        }
        items.push(CompletionItem {
            label: tn.to_string(),
            kind: Some(CompletionItemKind::TYPE_PARAMETER),
            sort_text: Some(format!("0-{}", tn)),
            ..CompletionItem::default()
        });
    }
}

fn add_statement_keywords(prefix: &str, items: &mut Vec<CompletionItem>) {
    for kw in &["SELECT", "CREATE", "UPDATE", "DELETE", "INSERT", "UPSERT",
                "RELATE", "LET", "IF", "FOR", "RETURN", "THROW", "BREAK", "CONTINUE"] {
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

fn extract_namespace_from_text(text: &str) -> Option<String> {
    if !text.ends_with("::") { return None; }
    let before = &text[..text.len() - 2];
    let start = before.rfind(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    let ns = &before[start..];
    if ns.is_empty() { None } else { Some(ns.to_string()) }
}

fn node_text<'a>(node: &tree_sitter::Node, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or("").trim()
}

/// Walk up from a node to find the enclosing statement and extract its target table.
fn find_table_from_ancestor(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let mut current = *node;
    while let Some(parent) = current.parent() {
        if parent.kind().ends_with("_statement") {
            return extract_table_from_statement(&parent, source);
        }
        current = parent;
    }
    None
}

/// Extract the primary table from a statement node.
fn extract_table_from_statement(node: &tree_sitter::Node, source: &str) -> Option<String> {
    crate::hover::extract_table_from_statement_pub(node, source)
}

/// Find table from DEFINE FIELD ... ON table ... ancestor
fn find_define_field_table_from_ancestor(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let mut current = *node;
    while let Some(parent) = current.parent() {
        if parent.kind() == "define_field_statement" {
            let mut c = parent.walk();
            let children: Vec<_> = parent.named_children(&mut c).collect();
            for child in &children {
                if child.kind() == "on_table_clause" {
                    let mut ic = child.walk();
                    for inner in child.named_children(&mut ic) {
                        if inner.kind() == "identifier" {
                            return Some(node_text(&inner, source).to_string());
                        }
                    }
                }
            }
        }
        current = parent;
    }
    None
}

/// Find table from ON clause keywords inside ERROR node children
fn find_define_field_table_in_children(children: &[tree_sitter::Node], source: &str) -> Option<String> {
    let mut after_on = false;
    for child in children {
        if child.kind() == "keyword_on" { after_on = true; continue; }
        if after_on && child.kind() == "keyword_table" { continue; } // skip TABLE keyword
        if after_on && child.kind() == "identifier" {
            return Some(node_text(child, source).to_string());
        }
    }
    None
}

/// Check if there's an = sign before the current node in a field_assignment
fn has_preceding_equals(parent: &tree_sitter::Node, current: &tree_sitter::Node, source: &str) -> bool {
    let mut c = parent.walk();
    let children: Vec<_> = parent.children(&mut c).collect();
    children.iter().any(|ch| {
        node_text(ch, source) == "=" && ch.start_byte() < current.start_byte()
    })
}

/// Check if node is inside a content_clause (walk up)
fn is_inside_content_clause(node: &tree_sitter::Node) -> bool {
    let mut current = *node;
    while let Some(parent) = current.parent() {
        if parent.kind() == "content_clause" { return true; }
        if parent.kind().ends_with("_statement") { return false; }
        current = parent;
    }
    false
}

/// Check if an identifier is a statement target (before any clause)
fn is_statement_target(statement: &tree_sitter::Node, ident: &tree_sitter::Node) -> bool {
    let mut c = statement.walk();
    let children: Vec<_> = statement.named_children(&mut c).collect();
    for child in &children {
        if child.kind().ends_with("_clause") { return false; }
        if child.id() == ident.id() { return true; }
    }
    false
}

/// Find preceding relation name in a graph path
fn find_preceding_relation_in_path(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let mut current = *node;
    while let Some(parent) = current.parent() {
        if parent.kind() == "path" {
            let mut c = parent.walk();
            let children: Vec<_> = parent.named_children(&mut c).collect();
            let our_idx = children.iter().position(|c| {
                c.start_byte() <= node.start_byte() && c.end_byte() >= node.end_byte()
            })?;
            if our_idx > 0 {
                return first_identifier_in(&children[our_idx - 1], source);
            }
            return None;
        }
        current = parent;
    }
    None
}

fn first_identifier_in(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let mut stack = vec![*node];
    while let Some(current) = stack.pop() {
        if current.kind() == "identifier" {
            return Some(node_text(&current, source).to_string());
        }
        let mut c = current.walk();
        for child in current.named_children(&mut c) {
            stack.push(child);
        }
    }
    None
}

/// Extract FROM table from raw text (for -> trigger)
fn find_from_table_in_text(text: &str) -> Option<String> {
    let upper = text.to_uppercase();
    // Find last FROM clause
    if let Some(from_pos) = upper.rfind("FROM ") {
        let after = &text[from_pos + 5..];
        let table = after.split_whitespace().next()?;
        let clean = table.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
        if !clean.is_empty() {
            return Some(clean.to_lowercase());
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
