/// Statement analysis — dispatches each statement type to its analyzer.
use tree_sitter::Node;

use crate::context::Context;
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{child_by_kind, find_all, node_text};
use crate::resolve::resolve_expr;
use crate::span::Span;
use crate::types::Kind;

pub mod select;
mod create;
mod update;
mod delete;
mod insert;
mod upsert;
mod relate;
mod define;
mod remove;
mod control;
mod info;
mod kill;
mod live_select;
mod show;
mod alter;

/// Analyze a single statement node and emit diagnostics into the context.
/// Returns the result type of the statement.
pub fn analyze_statement(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    match node.kind() {
        "select_statement" => select::analyze(node, source, ctx),
        "create_statement" => create::analyze(node, source, ctx),
        "update_statement" => update::analyze(node, source, ctx),
        "delete_statement" => delete::analyze(node, source, ctx),
        "insert_statement" => insert::analyze(node, source, ctx),
        "upsert_statement" => upsert::analyze(node, source, ctx),
        "relate_statement" => relate::analyze(node, source, ctx),
        "define_table_statement" | "define_field_statement" | "define_index_statement"
        | "define_event_statement" | "define_function_statement" | "define_param_statement"
        | "define_scope_statement" | "define_namespace_statement"
        | "define_database_statement" | "define_analyzer_statement"
        | "define_token_statement" | "define_user_statement"
        | "define_access_statement" | "define_api_statement"
        | "define_module_statement" | "define_bucket_statement"
        | "define_config_statement" => { define::analyze(node, source, ctx); Kind::Null },
        "if_statement" | "if_expression" => control::analyze_if(node, source, ctx),
        "for_statement" => { control::analyze_for(node, source, ctx); Kind::Null },
        "let_statement" => { control::analyze_let(node, source, ctx); Kind::Null },
        "return_statement" => { control::analyze_return(node, source, ctx); Kind::Any },
        "throw_statement" => { control::analyze_throw(node, source, ctx); Kind::Null },
        "block" | "block_expression" => { control::analyze_block(node, source, ctx); Kind::Any },
        "break_statement" => { control::analyze_break(node, source, ctx); Kind::Null },
        "continue_statement" => { control::analyze_continue(node, source, ctx); Kind::Null },
        "sleep_statement" => {
            // Resolve sleep argument expression
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if !child.kind().starts_with("keyword_") {
                    let typ = resolve_expr(&child, source, ctx, None);
                    if typ != Kind::Duration && typ != Kind::Any {
                        ctx.emit(Diagnostic::warning(
                            Span::from_node(&child),
                            Code::TypeMismatch,
                            format!("SLEEP expects a duration, got `{}`", typ),
                        ));
                    }
                }
            }
            Kind::Null
        }
        "begin_statement" => {
            ctx.transaction_depth += 1;
            Kind::Null
        }
        "commit_statement" | "cancel_statement" => {
            if ctx.transaction_depth == 0 {
                let keyword = if node.kind() == "commit_statement" { "COMMIT" } else { "CANCEL" };
                ctx.emit(Diagnostic::warning(
                    Span::from_node(node),
                    Code::UnbalancedTransaction,
                    format!("{} without a matching BEGIN transaction", keyword),
                ));
            } else {
                ctx.transaction_depth -= 1;
            }
            Kind::Null
        }
        "remove_statement" => remove::analyze(node, source, ctx),
        "info_statement" => info::analyze(node, source, ctx),
        "kill_statement" => kill::analyze(node, source, ctx),
        "show_statement" => show::analyze(node, source, ctx),
        "alter_statement" => alter::analyze(node, source, ctx),
        // Administrative / infrastructure statements — no type analysis needed
        "rebuild_index_statement"
        | "use_statement" | "option_statement" => Kind::Null,
        // LIVE SELECT returns a UUID (the live query ID) but we analyze the inner query
        "live_select_statement" | "live_select_diff_statement" => live_select::analyze(node, source, ctx),
        _ => {
            // Unknown statement — try resolving as expression for side effects
            resolve_expr(node, source, ctx, None)
        }
    }
}

/// Analyze all statements in a source file.
/// Walks the tree recursively to find statement nodes inside wrapper nodes
/// like `expressions`, `expression`, `subquery_statement`.
pub fn analyze_all(root: &Node, source: &str, ctx: &mut Context) {
    // Reset transaction depth before analysis
    ctx.transaction_depth = 0;

    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        analyze_node_recursive(&child, source, ctx);
    }

    // Check for unclosed transactions at the end of the file
    if ctx.transaction_depth > 0 {
        let msg = if ctx.transaction_depth == 1 {
            "BEGIN transaction is never closed with COMMIT or CANCEL".to_string()
        } else {
            format!(
                "{} BEGIN transactions are never closed with COMMIT or CANCEL",
                ctx.transaction_depth
            )
        };
        ctx.emit(Diagnostic::warning(
            Span::new(0, source.len() as u32),
            Code::UnbalancedTransaction,
            msg,
        ));
        ctx.transaction_depth = 0;
    }
}

fn analyze_node_recursive(node: &Node, source: &str, ctx: &mut Context) {
    let kind = node.kind();
    // If it's a recognized statement (but NOT a wrapper), analyze it directly
    if kind.ends_with("_statement") && kind != "subquery_statement" && kind != "primary_statement" {
        analyze_statement(node, source, ctx);
        return;
    }
    if kind == "block" || kind == "block_expression" {
        analyze_statement(node, source, ctx);
        return;
    }
    // Otherwise, descend into wrapper nodes
    match kind {
        "expressions" | "expression" | "subquery_statement" | "primary_statement" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                analyze_node_recursive(&child, source, ctx);
            }
        }
        _ => {
            // Try as expression for side effects
            resolve_expr(node, source, ctx, None);
        }
    }
}

/// Extract the target table name from a FROM clause.
pub(crate) fn extract_from_table<'a>(node: &Node<'a>, source: &'a str) -> Option<String> {
    // Look for the source table in FROM clause.
    // For graph traversals like `FROM user:1->wrote->post`, we need the source table (`user`),
    // NOT the edge or target tables. So we avoid descending into graph_path/path_element nodes.
    if let Some(from) = child_by_kind(node, "from_clause") {
        if let Some(result) = extract_from_source_shallow(&from, source) {
            return Some(result);
        }
    }
    // Some statements have the table directly
    let mut cursor = node.walk();
    let mut found_from = false;
    for child in node.children(&mut cursor) {
        if child.kind() == "keyword_from" {
            found_from = true;
        } else if found_from && child.kind() == "identifier" {
            return Some(node_text(&child, source).to_string());
        }
    }
    None
}

/// Extract the source table from a FROM clause without descending into graph paths.
/// Walks shallowly through value/base_value/path wrappers but stops at graph_path/path_element.
fn extract_from_source_shallow(node: &Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => return Some(node_text(&child, source).to_string()),
            "record_id" => return extract_table_from_record_id(&child, source),
            // Skip keywords and punctuation, descend into value wrappers
            k if k.starts_with("keyword_") => continue,
            "value" | "base_value" | "path" | "inclusive_predicate" => {
                if let Some(result) = extract_from_source_shallow(&child, source) {
                    return Some(result);
                }
            }
            // Don't descend into graph paths or path elements - those contain edge/target tables
            "graph_path" | "path_element" | "graph_predicate" | "graph_expression" => continue,
            _ => continue,
        }
    }
    None
}

/// Extract the target table from statements that use a direct target (CREATE, UPDATE, etc.)
/// Handles tree-sitter wrappers like `create_target`, `value`, `base_value`.
pub(crate) fn extract_target_table<'a>(node: &Node<'a>, source: &'a str) -> Option<String> {
    // First, look for a create_target or similar wrapper node
    for wrapper_kind in &["create_target", "update_target", "delete_target", "upsert_target"] {
        if let Some(target) = child_by_kind(node, wrapper_kind) {
            return extract_identifier_deep(&target, source);
        }
    }
    // Fallback: look for identifier/record_id after keyword
    let mut cursor = node.walk();
    let mut past_keyword = false;
    for child in node.children(&mut cursor) {
        let kind = child.kind();
        if kind.starts_with("keyword_") {
            past_keyword = true;
        } else if past_keyword {
            if kind == "identifier" {
                return Some(node_text(&child, source).to_string());
            }
            if kind == "record_id" {
                return extract_table_from_record_id(&child, source);
            }
            // Try recursing into value/base_value wrappers
            if kind == "value" || kind == "base_value" {
                return extract_identifier_deep(&child, source);
            }
        }
    }
    None
}

/// Recursively find the first identifier or record_id table name in a node tree.
fn extract_identifier_deep<'a>(node: &Node<'a>, source: &'a str) -> Option<String> {
    let kind = node.kind();
    if kind == "identifier" {
        return Some(node_text(node, source).to_string());
    }
    if kind == "record_id" {
        return extract_table_from_record_id(node, source);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(result) = extract_identifier_deep(&child, source) {
            return Some(result);
        }
    }
    None
}

/// Extract table name from a record_id node (e.g., "user:123" → "user").
fn extract_table_from_record_id<'a>(node: &Node<'a>, source: &'a str) -> Option<String> {
    // record_id has object_key child with the table name
    if let Some(key) = child_by_kind(node, "object_key") {
        return Some(node_text(&key, source).to_string());
    }
    let text = node_text(node, source);
    if let Some(colon_pos) = text.find(':') {
        Some(text[..colon_pos].to_string())
    } else {
        Some(text.to_string())
    }
}

/// Extract the assignment operator text from a field_assignment node.
///
/// The tree-sitter CST for `field_assignment` contains an `assignment_operator`
/// named child with text `=`, `+=`, `-=`, or `+?=`.
/// Returns `"="` if no operator node is found (backwards compatibility).
pub(crate) fn extract_assignment_operator<'a>(node: &Node<'a>, source: &'a str) -> &'a str {
    if let Some(op_node) = child_by_kind(node, "assignment_operator") {
        node_text(&op_node, source).trim()
    } else {
        "="
    }
}

/// Information about a field path extracted from a field_assignment node.
///
/// A field_assignment can have either:
/// - An `identifier` child (simple field like `name`)
/// - A `path` child (nested field like `name.first` or `tags[0]`)
pub(crate) struct FieldPathInfo {
    /// The top-level field name (e.g., "name" for both `name` and `name.first`)
    pub top_level: String,
    /// The full dotted path (e.g., "name.first") — same as `top_level` for simple fields
    pub full_path: String,
    /// Whether this is a nested path (has dotted subscripts)
    pub has_subscripts: bool,
    /// Whether this path contains array indexing (filter/bracket access)
    pub has_array_index: bool,
}

/// Extract field path information from a field_assignment node.
///
/// Handles both simple identifiers (`name`) and path nodes (`name.first`, `tags[0]`).
pub(crate) fn extract_field_path_info(node: &Node, source: &str) -> Option<FieldPathInfo> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                let name = node_text(&child, source).to_string();
                return Some(FieldPathInfo {
                    top_level: name.clone(),
                    full_path: name,
                    has_subscripts: false,
                    has_array_index: false,
                });
            }
            "path" => {
                return extract_path_info(&child, source);
            }
            _ => {}
        }
    }
    None
}

/// Extract path information from a `path` node.
fn extract_path_info(node: &Node, source: &str) -> Option<FieldPathInfo> {
    let mut top_level = None;
    let mut subscript_names = Vec::new();
    let mut has_array_index = false;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "base_value" => {
                // The base_value contains the top-level identifier
                let idents = find_all(&child, "identifier");
                if let Some(ident) = idents.first() {
                    top_level = Some(node_text(ident, source).to_string());
                }
            }
            "path_element" => {
                let mut inner_cursor = child.walk();
                for inner in child.children(&mut inner_cursor) {
                    match inner.kind() {
                        "subscript" => {
                            // subscript: '.', identifier
                            let idents = find_all(&inner, "identifier");
                            if let Some(ident) = idents.first() {
                                subscript_names.push(node_text(ident, source).to_string());
                            }
                        }
                        "filter" => {
                            has_array_index = true;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    let top = top_level?;
    let has_subscripts = !subscript_names.is_empty();
    let full_path = if subscript_names.is_empty() {
        top.clone()
    } else {
        format!("{}.{}", top, subscript_names.join("."))
    };

    Some(FieldPathInfo {
        top_level: top,
        full_path,
        has_subscripts,
        has_array_index,
    })
}

/// Validate nested field path access on a known field type.
///
/// If the top-level field type is known and not compatible with nested access
/// (e.g., setting `name.first` where `name` is `string`), emit a warning.
///
/// Array indexing (e.g., `tags[0]`) on array types is always allowed.
pub(crate) fn validate_nested_field_path(
    node: &Node,
    ctx: &mut Context,
    table: &str,
    path_info: &FieldPathInfo,
) {
    use crate::types::KindExt;

    // Only validate if there are nested components
    if !path_info.has_subscripts && !path_info.has_array_index {
        return;
    }

    // Only validate on schemafull tables — schemaless tables are permissive
    if !ctx.is_schemafull(table) {
        return;
    }

    let field_def = match ctx.get_field(table, &path_info.top_level) {
        Some(f) => f,
        None => return, // Unknown field — nothing to validate
    };

    let field_type = match &field_def.typ {
        Some(t) => t.clone(),
        None => return, // No type info
    };

    // If the field is Any, Object, or an object literal, nested access is fine
    if field_type.is_any() || matches!(field_type, Kind::Object | Kind::Literal(Literal::Object(_))) {
        return;
    }

    // If accessing array elements on an array type, that's valid
    if path_info.has_array_index && !path_info.has_subscripts {
        if matches!(field_type, Kind::Array(_, _) | Kind::Set(_, _)) {
            return;
        }
    }

    // If the type is Option<T>, check the inner type
    if let Kind::Option(inner) = &field_type {
        if inner.is_any() || matches!(**inner, Kind::Object | Kind::Literal(Literal::Object(_))) {
            return;
        }
        if path_info.has_array_index && !path_info.has_subscripts {
            if matches!(**inner, Kind::Array(_, _) | Kind::Set(_, _)) {
                return;
            }
        }
    }

    // If there are subscripts (dotted access) on a non-object type, warn
    if path_info.has_subscripts {
        let span = Span::from_node(node);
        ctx.emit(Diagnostic::warning(
            span,
            Code::InvalidNestedAccess,
            format!(
                "cannot access nested field `{}` on field `{}` of type `{}`",
                path_info.full_path, path_info.top_level, field_type
            ),
        )
        .with_related(field_def.span, format!("`{}` defined as `{}` here", path_info.top_level, field_type))
        .with_suggestion(format!("if `{}` should support nested fields, change its type to `object`", path_info.top_level)));
    }
}

// ── Duplicate Field Assignment Check ─────────────────────────

/// Check for duplicate field assignments in SET clauses.
///
/// When a SET clause assigns the same field more than once (e.g.,
/// `SET name = 'John', name = 'Jane'`), the later assignment silently
/// overwrites the earlier one. This emits a warning for each duplicate.
pub(crate) fn check_duplicate_field_assignments(node: &Node, source: &str, ctx: &mut Context) {
    use std::collections::HashMap;

    let set_clauses = find_all(node, "set_clause");
    for set_clause in &set_clauses {
        let assignments = find_all(set_clause, "field_assignment");
        let mut seen: HashMap<String, Span> = HashMap::new();

        for assignment in &assignments {
            if let Some(path_info) = extract_field_path_info(assignment, source) {
                let field_name = path_info.full_path;
                let span = Span::from_node(assignment);
                if let Some(first_span) = seen.get(&field_name) {
                    ctx.emit(
                        Diagnostic::warning(
                            span,
                            Code::DuplicateFieldAssignment,
                            format!("duplicate assignment to field `{}`", field_name),
                        )
                        .with_related(*first_span, "first assignment here".to_string()),
                    );
                } else {
                    seen.insert(field_name, span);
                }
            }
        }
    }

    // Also check for direct field_assignment children (some grammars put them directly)
    let mut direct_assignments = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "field_assignment" {
            direct_assignments.push(child);
        }
    }
    if !direct_assignments.is_empty() && set_clauses.is_empty() {
        let mut seen: std::collections::HashMap<String, Span> = std::collections::HashMap::new();
        for assignment in &direct_assignments {
            if let Some(path_info) = extract_field_path_info(assignment, source) {
                let field_name = path_info.full_path;
                let span = Span::from_node(assignment);
                if let Some(first_span) = seen.get(&field_name) {
                    ctx.emit(
                        Diagnostic::warning(
                            span,
                            Code::DuplicateFieldAssignment,
                            format!("duplicate assignment to field `{}`", field_name),
                        )
                        .with_related(*first_span, "first assignment here".to_string()),
                    );
                } else {
                    seen.insert(field_name, span);
                }
            }
        }
    }
}

// ── PATCH Clause Validation ──────────────────────────────────

/// Valid JSON Patch operations per RFC 6902.
const VALID_PATCH_OPS: &[&str] = &["add", "remove", "replace", "move", "copy", "test"];

/// Validate the structure of a PATCH clause value.
///
/// When the PATCH value is a literal array of literal objects, we validate:
/// - Each element should be an object
/// - Each object should have an `op` field
/// - If `op` is a string literal, it must be one of the valid RFC 6902 operations
///
/// Dynamic values (variables, expressions) are not validated since their
/// structure cannot be determined statically.
pub(crate) fn validate_patch_syntax(node: &Node, source: &str, ctx: &mut Context) {
    // node is a patch_clause; find the array child
    let array_node = match child_by_kind(node, "array") {
        Some(n) => n,
        None => return,
    };

    // Iterate over the value children of the array
    let mut cursor = array_node.walk();
    for child in array_node.named_children(&mut cursor) {
        let inner = unwrap_value_node(&child);
        match inner.kind() {
            "object" => {
                validate_patch_object(&inner, source, ctx);
            }
            "variable_name" => {
                // Dynamic value -- cannot validate statically, skip
            }
            _ => {
                let span = Span::from_node(&inner);
                ctx.emit(Diagnostic::warning(
                    span,
                    Code::InvalidPatchOperation,
                    format!(
                        "PATCH array element should be an object, got `{}`",
                        inner.kind()
                    ),
                ));
            }
        }
    }
}

/// Validate a single object within a PATCH array.
/// Checks for the presence and validity of the `op` field.
fn validate_patch_object(node: &Node, source: &str, ctx: &mut Context) {
    let mut found_op = false;

    // Iterate through object properties
    let properties = find_all(node, "object_property");
    for prop in &properties {
        let mut key_name = None;
        let mut value_node = None;
        let mut prop_cursor = prop.walk();
        for child in prop.children(&mut prop_cursor) {
            match child.kind() {
                "object_key" => {
                    key_name = Some(node_text(&child, source).to_string());
                }
                "string" | "plain_string" | "strand" => {
                    // In SurrealQL objects, keys can be quoted strings like "op"
                    // Check if this is the key position (before ':') by checking
                    // if we haven't found a key yet
                    if key_name.is_none() && value_node.is_none() {
                        let text = node_text(&child, source);
                        let unquoted = text
                            .trim_start_matches(|c| c == '"' || c == '\'')
                            .trim_end_matches(|c| c == '"' || c == '\'');
                        key_name = Some(unquoted.to_string());
                    }
                }
                "value" | "base_value" => {
                    value_node = Some(child);
                }
                _ => {}
            }
        }

        if let Some(key) = key_name {
            if key == "op" {
                found_op = true;
                if let Some(val) = value_node {
                    let inner = unwrap_value_node(&val);
                    // Check if it's a string literal
                    let text = node_text(&inner, source);
                    if inner.kind() == "string" || inner.kind() == "plain_string" || inner.kind() == "strand" {
                        // Strip quotes
                        let unquoted = text
                            .trim_start_matches(|c| c == '"' || c == '\'')
                            .trim_end_matches(|c| c == '"' || c == '\'');
                        if !VALID_PATCH_OPS.contains(&unquoted) {
                            let span = Span::from_node(&inner);
                            ctx.emit(Diagnostic::warning(
                                span,
                                Code::InvalidPatchOperation,
                                format!(
                                    "invalid PATCH operation `{}`, expected one of: {}",
                                    unquoted,
                                    VALID_PATCH_OPS.join(", ")
                                ),
                            ));
                        }
                    }
                    // If it's a variable or expression, we can't validate statically
                }
            }
        }
    }

    if !found_op {
        let span = Span::from_node(node);
        ctx.emit(Diagnostic::warning(
            span,
            Code::InvalidPatchOperation,
            "PATCH object is missing required `op` field".to_string(),
        ));
    }
}

/// Unwrap value/base_value wrappers to get the inner node.
fn unwrap_value_node<'a>(node: &Node<'a>) -> Node<'a> {
    match node.kind() {
        "value" | "base_value" => {
            let mut cursor = node.walk();
            if let Some(child) = node.named_children(&mut cursor).next() {
                return unwrap_value_node(&child);
            }
            *node
        }
        _ => *node,
    }
}

// ── Content Object Schema Validation ─────────────────────────

/// Validate an object literal against a schemafull table's schema.
///
/// Checks that:
/// 1. Every key in the object exists as a field on the table
/// 2. Every value is type-compatible with the field's declared type
///
/// This is the shared validation used by CONTENT clauses across
/// CREATE, UPDATE, UPSERT, INSERT, and RELATE statements.
pub(crate) fn validate_object_against_schema(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    table: &str,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "object_content" {
            let mut inner = child.walk();
            for prop in child.named_children(&mut inner) {
                if prop.kind() == "object_property" {
                    validate_object_property_against_schema(&prop, source, ctx, table);
                }
            }
        } else if child.kind() == "object_property" {
            validate_object_property_against_schema(&child, source, ctx, table);
        }
    }
}

/// Validate a single object property against the table schema.
/// Checks field existence and value type compatibility.
fn validate_object_property_against_schema(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    table: &str,
) {
    let mut key_name = None;
    let mut key_span = None;
    let mut value_node = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "object_key" => {
                key_name = Some(node_text(&child, source).to_string());
                key_span = Some(Span::from_node(&child));
            }
            "value" | "base_value" => {
                value_node = Some(child);
            }
            _ => {}
        }
    }

    let key = match key_name {
        Some(k) => k,
        None => return,
    };
    let span = key_span.unwrap();

    let field_info = ctx.get_field(table, &key).map(|f| (f.typ.clone(), f.span));
    if field_info.is_none() {
        let table_span = ctx.get_table(table).map(|t| t.span);
        let mut diag = Diagnostic::warning(
            span,
            Code::UndefinedFieldOnSchemafull,
            format!(
                "field `{}` is not defined on table `{}`",
                key, table
            ),
        )
        .with_suggestion(format!("consider adding `DEFINE FIELD {} ON {} TYPE ...`", key, table));
        if let Some(tbl_span) = table_span {
            diag = diag.with_related(tbl_span, format!("table `{}` defined as SCHEMAFULL here", table));
        }
        ctx.emit(diag);
    } else if let (Some((Some(expected_type), field_span)), Some(val_node)) = (&field_info, value_node) {
        let actual_type =
            crate::resolve::resolve_expr(&val_node, source, ctx, Some(table));
        if actual_type != crate::types::Kind::Any
            && !crate::types::is_assignable(expected_type, &actual_type)
        {
            let val_span = Span::from_node(&val_node);
            ctx.emit(Diagnostic::error(
                val_span,
                Code::IncompatibleAssignment,
                format!(
                    "expected `{}`, found `{}` in assignment to field `{}`",
                    expected_type, actual_type, key
                ),
            )
            .with_related(*field_span, format!("`{}` defined as `{}` here", key, expected_type)));
        }
    }
}

// ── DML Result Type Helpers ──────────────────────────────────

use std::collections::BTreeMap;
use crate::types::Literal;

/// Compute the result type for a DML statement (CREATE/UPDATE/UPSERT/INSERT/DELETE/RELATE).
///
/// The default result is `array<table_schema>`. This is modified by:
/// - ONLY modifier → unwraps array to single record
/// - RETURN NONE → Kind::Null
/// - RETURN BEFORE/AFTER → array<table_schema> (same as default)
/// - RETURN DIFF → array<object> (JSON patch operations)
/// - RETURN field1, field2 → array<projected_object>
pub(crate) fn compute_dml_result_type(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
) -> Kind {
    let is_only = !find_all(node, "keyword_only").is_empty();

    // Build the default table type
    let table_type = table
        .and_then(|tbl| ctx.build_table_type(tbl))
        .unwrap_or(Kind::Any);

    // Check for RETURN clause
    let result_type = if let Some(return_clause) = find_all(node, "return_clause").into_iter().next() {
        resolve_return_clause(&return_clause, source, ctx, table, &table_type)
    } else {
        table_type
    };

    // RETURN NONE always returns Null, regardless of ONLY
    if result_type == Kind::Null {
        return Kind::Null;
    }

    // Wrap in array unless ONLY
    if is_only {
        result_type
    } else {
        Kind::Array(Box::new(result_type), None)
    }
}

/// Resolve the type produced by a RETURN clause.
///
/// return_clause children:
/// - keyword_return
/// - keyword_before → pre-modification record (same schema)
/// - keyword_after → post-modification record (same schema, default)
/// - keyword_diff → JSON Patch array
/// - value, value, ... → projected fields
fn resolve_return_clause(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
    table_type: &Kind,
) -> Kind {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.named_children(&mut cursor).collect();

    for child in &children {
        match child.kind() {
            "keyword_return" => continue,
            "keyword_before" | "keyword_after" => return table_type.clone(),
            "keyword_diff" => {
                // RETURN DIFF produces JSON Patch operations
                let mut patch_fields = BTreeMap::new();
                patch_fields.insert("op".to_string(), Kind::String);
                patch_fields.insert("path".to_string(), Kind::String);
                patch_fields.insert("value".to_string(), Kind::Any);
                return Kind::Literal(Literal::Object(patch_fields));
            }
            "keyword_none" => return Kind::Null,
            // Value expressions → field projection or NONE literal
            "value" | "base_value" => {
                let text = node_text(child, source).trim();
                // Check if it's NONE
                if text.eq_ignore_ascii_case("none") {
                    return Kind::Null;
                }
            }
            _ => {}
        }
    }

    // If we get here, it's RETURN field1, field2, ...
    // Collect the field expressions
    let field_values: Vec<&Node> = children
        .iter()
        .filter(|c| {
            c.kind() != "keyword_return"
                && c.kind() != "keyword_value"
                && !c.kind().starts_with("keyword_")
        })
        .collect();

    if field_values.is_empty() {
        return table_type.clone();
    }

    // Build projected object type from the field list
    let mut fields = BTreeMap::new();
    for field_node in &field_values {
        let typ = resolve_expr(field_node, source, ctx, table);
        // Extract the field name (identifier)
        let idents = find_all(field_node, "identifier");
        if let Some(ident) = idents.first() {
            let name = node_text(ident, source).to_string();
            fields.insert(name, typ);
        }
    }

    if fields.is_empty() {
        table_type.clone()
    } else {
        Kind::Literal(Literal::Object(fields))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    #[test]
    fn extract_create_target() {
        let src = "CREATE user SET name = 'test';";
        let tree = parser::parse(src).unwrap();
        let stmts = parser::find_all(&tree.root_node(), "create_statement");
        assert_eq!(stmts.len(), 1);
        let table = extract_target_table(&stmts[0], src);
        assert_eq!(table.as_deref(), Some("user"), "Should extract 'user' from CREATE");
    }

    #[test]
    fn extract_update_target() {
        let src = "UPDATE user SET age = 30;";
        let tree = parser::parse(src).unwrap();
        let stmts = parser::find_all(&tree.root_node(), "update_statement");
        assert_eq!(stmts.len(), 1);
        let table = extract_target_table(&stmts[0], src);
        assert_eq!(table.as_deref(), Some("user"), "Should extract 'user' from UPDATE");
    }

    #[test]
    fn extract_from_select() {
        let src = "SELECT name FROM user;";
        let tree = parser::parse(src).unwrap();
        let stmts = parser::find_all(&tree.root_node(), "select_statement");
        assert_eq!(stmts.len(), 1);
        let table = extract_from_table(&stmts[0], src);
        assert_eq!(table.as_deref(), Some("user"), "Should extract 'user' from SELECT");
    }

    #[test]
    fn remove_statement_no_error() {
        let src = "REMOVE TABLE user;";
        let tree = parser::parse(src).unwrap();
        let mut ctx = crate::Context::new();
        analyze_all(&tree.root_node(), src, &mut ctx);
        assert!(ctx.diagnostics.is_empty(), "REMOVE TABLE should not produce diagnostics");
    }

    #[test]
    fn info_statement_no_error() {
        let src = "INFO FOR DB;";
        let tree = parser::parse(src).unwrap();
        let mut ctx = crate::Context::new();
        analyze_all(&tree.root_node(), src, &mut ctx);
        assert!(ctx.diagnostics.is_empty(), "INFO FOR DB should not produce diagnostics");
    }

    #[test]
    fn begin_without_commit_warns() {
        let result = crate::analyze(r#"
            BEGIN;
            CREATE user SET name = 'test';
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.message.contains("transaction") || d.message.contains("BEGIN"))
            .collect();
        assert!(!warnings.is_empty(), "Expected warning for unclosed BEGIN, got: {:?}", result.diagnostics);
    }

    #[test]
    fn balanced_transaction_no_warning() {
        let result = crate::analyze(r#"
            BEGIN;
            CREATE user SET name = 'test';
            COMMIT;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.message.contains("transaction"))
            .collect();
        assert!(warnings.is_empty(), "Balanced transaction should not warn: {:?}", warnings);
    }

    #[test]
    fn commit_without_begin_warns() {
        let result = crate::analyze(r#"
            CREATE user SET name = 'test';
            COMMIT;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.message.contains("transaction") || d.message.contains("COMMIT"))
            .collect();
        assert!(!warnings.is_empty(), "Expected warning for orphaned COMMIT, got: {:?}", result.diagnostics);
    }

    #[test]
    fn cancel_without_begin_warns() {
        let result = crate::analyze(r#"
            CREATE user SET name = 'test';
            CANCEL;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.message.contains("transaction") || d.message.contains("CANCEL"))
            .collect();
        assert!(!warnings.is_empty(), "Expected warning for orphaned CANCEL, got: {:?}", result.diagnostics);
    }

    #[test]
    fn balanced_transaction_with_cancel_no_warning() {
        let result = crate::analyze(r#"
            BEGIN;
            CREATE user SET name = 'test';
            CANCEL;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.message.contains("transaction"))
            .collect();
        assert!(warnings.is_empty(), "BEGIN + CANCEL should be balanced: {:?}", warnings);
    }

    #[test]
    fn use_statement_no_error() {
        let src = "USE NS test DB test;";
        let tree = parser::parse(src).unwrap();
        let mut ctx = crate::Context::new();
        analyze_all(&tree.root_node(), src, &mut ctx);
        assert!(ctx.diagnostics.is_empty(), "USE NS DB should not produce diagnostics");
    }

    #[test]
    fn analyze_all_finds_statements() {
        let src = r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            CREATE user SET name = 'test';
        "#;
        let tree = parser::parse(src).unwrap();
        let mut ctx = crate::Context::new();
        crate::schema::extract_schema(&tree.root_node(), src, &mut ctx);
        analyze_all(&tree.root_node(), src, &mut ctx);
        // Should not panic and should process without errors
    }
}
