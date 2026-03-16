use std::collections::{BTreeMap, HashSet};
use tree_sitter::Node;

use crate::context::Context;
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{child_by_kind, find_all, node_text};
use crate::resolve::resolve_expr;
use crate::span::Span;
use crate::types::{Kind, KindExt, Literal};

/// Analyze a SELECT statement and return its result type.
///
/// The result type accounts for:
/// - SELECT fields (specific fields vs `*`)
/// - VALUE modifier (returns single field type instead of object)
/// - ONLY modifier (returns single record instead of array)
/// - OMIT clause (removes fields from result)
/// - FETCH clause (expands record links to full table schemas)
pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    let table = super::extract_from_table(node, source);
    let table_ref = table.as_deref();

    // Validate table exists (strict mode)
    if let Some(tbl) = table_ref {
        if ctx.strict && !ctx.has_table(tbl) {
            if let Some(from) = child_by_kind(node, "from_clause") {
                // Span the table identifier within the FROM clause, not the entire clause
                let span = find_all(&from, "identifier")
                    .first()
                    .map(|id| Span::from_node(id))
                    .unwrap_or_else(|| Span::from_node(&from));
                ctx.emit(Diagnostic::error(
                    span,
                    Code::TableNotFound,
                    format!("table `{}` not defined", tbl),
                ));
            }
        }
    }

    // Validate WITH INDEX clause: check that referenced indexes exist on the table.
    // The with_clause lives inside from_clause, so use find_all for recursive search.
    if let Some(with_clause) = find_all(node, "with_clause").into_iter().next() {
        if child_by_kind(&with_clause, "keyword_index").is_some() {
            let index_names = find_all(&with_clause, "identifier");
            if let Some(tbl) = table_ref {
                for idx_node in &index_names {
                    let idx_name = node_text(idx_node, source);
                    if !ctx.has_index(tbl, idx_name) {
                        let span = Span::from_node(idx_node);
                        ctx.emit(Diagnostic::error(
                            span,
                            Code::IndexNotFound,
                            format!(
                                "index `{}` not defined on table `{}`",
                                idx_name, tbl
                            ),
                        ));
                    }
                }
            }
        }
    }

    // Check for ONLY keyword (inside from_clause)
    let is_only = !find_all(node, "keyword_only").is_empty();

    // Check for VALUE keyword
    let is_value = has_value_keyword(node);

    // Warn if SELECT VALUE is used with multiple fields.
    // The tree-sitter grammar only allows one field after VALUE, so extra
    // fields appear as an ERROR node (e.g. ", age") at the statement level.
    if is_value {
        let mut err_cursor = node.walk();
        for child in node.children(&mut err_cursor) {
            if child.kind() == "ERROR" {
                let err_text = node_text(&child, source).trim();
                if err_text.starts_with(',') {
                    let span = Span::from_node(&child);
                    ctx.emit(Diagnostic::warning(
                        span,
                        Code::SelectValueMultipleFields,
                        "SELECT VALUE with multiple fields will only return the first field's values",
                    ));
                    break;
                }
            }
        }
    }

    // Detect OMIT clause
    let omit_fields = extract_omit_fields(node, source);

    // Validate OMIT fields exist on the table (schemafull only)
    if let Some(tbl) = table_ref {
        if let Some(table_def) = ctx.get_table(tbl) {
            if table_def.schema_mode == crate::context::SchemaMode::Schemafull {
                if let Some(omit_node) = child_by_kind(node, "omit_clause") {
                    let omit_idents = find_all(&omit_node, "identifier");
                    for (i, field) in omit_fields.iter().enumerate() {
                        if ctx.get_field(tbl, field).is_none() && field != "id" {
                            let span = omit_idents.get(i)
                                .map(|n| Span::from_node(n))
                                .unwrap_or_else(|| Span::from_node(&omit_node));
                            ctx.emit(Diagnostic::warning(
                                span,
                                Code::FieldNotFound,
                                format!(
                                    "field `{}` in OMIT clause not defined on table `{}`",
                                    field, tbl
                                ),
                            ));
                        }
                    }
                }
            }
        }
    }

    // Detect FETCH clause
    let fetch_fields = extract_fetch_fields(node, source);

    // Build the result type from SELECT fields
    let mut result_type = Kind::Any;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "select_clause" | "field_list" | "select_fields" => {
                result_type =
                    build_select_type(&child, source, ctx, table_ref, is_value, &omit_fields);
            }
            "from_clause" => {
                // Clauses like LIMIT, START, TIMEOUT, SPLIT, etc. are children of from_clause
                analyze_from_subclauses(&child, source, ctx, table_ref);
            }
            "where_clause" => {
                analyze_where(&child, source, ctx, table_ref);
            }
            _ => {
                // Handle clauses that might be direct children of select_statement
                analyze_select_clause(&child, source, ctx, table_ref);
            }
        }
    }

    // Validate GROUP BY has aggregate functions in SELECT
    {
        let group_nodes = find_all(node, "group_clause");
        if let Some(group_node) = group_nodes.first() {
            validate_group_by_has_aggregates(node, group_node, source, ctx);
        }
    }

    // Validate FETCH fields are record types
    if !fetch_fields.is_empty() {
        if let Some(tbl) = table_ref {
            let fetch_nodes = find_all(node, "fetch_clause");
            if let Some(fetch_node) = fetch_nodes.first() {
                let fetch_idents = find_all(fetch_node, "identifier");
                for (i, field) in fetch_fields.iter().enumerate() {
                    if let Some(field_def) = ctx.get_field(tbl, field) {
                        if let Some(ref typ) = field_def.typ {
                            if !typ.is_record() && *typ != Kind::Any {
                                let span = fetch_idents.get(i)
                                    .map(|n| Span::from_node(n))
                                    .unwrap_or_else(|| Span::from_node(fetch_node));
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

    // Apply FETCH expansion (expand record links to full table types)
    if !fetch_fields.is_empty() {
        result_type = ctx.expand_record_links(&result_type, &fetch_fields);
    }

    // Wrap in array unless ONLY
    if is_value {
        // SELECT VALUE returns the field type directly (or array of it)
        if is_only {
            result_type
        } else {
            Kind::Array(Box::new(result_type), None)
        }
    } else if is_only {
        result_type
    } else {
        Kind::Array(Box::new(result_type), None)
    }
}

/// Process subclauses within the FROM clause (LIMIT, START, TIMEOUT, SPLIT, etc.)
/// Also resolves the FROM value expression (e.g., graph traversals).
fn analyze_from_subclauses(node: &Node, source: &str, ctx: &mut Context, table_ref: Option<&str>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "value" | "path" => {
                // Resolve the FROM source expression (handles graph traversals, etc.)
                resolve_expr(&child, source, ctx, table_ref);
            }
            _ => {
                analyze_select_clause(&child, source, ctx, table_ref);
            }
        }
    }
}

/// Analyze a single SELECT subclause (LIMIT, START, TIMEOUT, SPLIT, ORDER, GROUP).
fn analyze_select_clause(node: &Node, source: &str, ctx: &mut Context, table_ref: Option<&str>) {
    match node.kind() {
        "where_clause" => {
            analyze_where(node, source, ctx, table_ref);
        }
        "order_clause" => {
            let mut inner = node.walk();
            for c in node.named_children(&mut inner) {
                resolve_expr(&c, source, ctx, table_ref);
            }
            // Validate ORDER BY fields exist on schemafull tables
            validate_clause_fields(node, "ORDER BY", source, ctx, table_ref);
        }
        "group_clause" => {
            let mut inner = node.walk();
            for c in node.named_children(&mut inner) {
                resolve_expr(&c, source, ctx, table_ref);
            }
            // Validate GROUP BY fields exist on schemafull tables
            validate_clause_fields(node, "GROUP BY", source, ctx, table_ref);
        }
        "limit_clause" => {
            let mut inner = node.walk();
            for c in node.named_children(&mut inner) {
                if c.kind().starts_with("keyword_") {
                    continue;
                }
                let typ = resolve_expr(&c, source, ctx, table_ref);
                if typ != Kind::Any && !typ.is_numeric() {
                    let span = Span::from_node(&c);
                    ctx.emit(Diagnostic::error(
                        span,
                        Code::TypeMismatch,
                        format!("LIMIT must be a numeric value, got `{}`", typ),
                    ));
                }
            }
        }
        "start_clause" => {
            let mut inner = node.walk();
            for c in node.named_children(&mut inner) {
                if c.kind().starts_with("keyword_") {
                    continue;
                }
                let typ = resolve_expr(&c, source, ctx, table_ref);
                if typ != Kind::Any && !typ.is_numeric() {
                    let span = Span::from_node(&c);
                    ctx.emit(Diagnostic::error(
                        span,
                        Code::TypeMismatch,
                        format!("START must be a numeric value, got `{}`", typ),
                    ));
                }
            }
        }
        "timeout_clause" => {
            let mut inner = node.walk();
            for c in node.named_children(&mut inner) {
                if c.kind().starts_with("keyword_") {
                    continue;
                }
                let typ = resolve_expr(&c, source, ctx, table_ref);
                if typ != Kind::Any && typ != Kind::Duration {
                    let span = Span::from_node(&c);
                    ctx.emit(Diagnostic::error(
                        span,
                        Code::TypeMismatch,
                        format!("TIMEOUT must be a duration, got `{}`", typ),
                    ));
                }
            }
        }
        "split_clause" => {
            let mut inner = node.walk();
            for c in node.named_children(&mut inner) {
                if c.kind().starts_with("keyword_") {
                    continue;
                }
                let typ = resolve_expr(&c, source, ctx, table_ref);
                if let Some(tbl) = table_ref {
                    let field_name = node_text(&c, source);
                    // Check field existence on schemafull tables
                    if let Some(table_def) = ctx.get_table(tbl) {
                        if table_def.schema_mode == crate::context::SchemaMode::Schemafull
                            && ctx.get_field(tbl, field_name).is_none()
                            && field_name != "id"
                        {
                            let span = Span::from_node(&c);
                            ctx.emit(Diagnostic::warning(
                                span,
                                Code::FieldNotFound,
                                format!(
                                    "field `{}` in SPLIT clause not defined on table `{}`",
                                    field_name, tbl
                                ),
                            ));
                        }
                    }
                    // Check that the field is an array type
                    if typ != Kind::Any && !matches!(typ, Kind::Array(_, _)) {
                        if ctx.get_field(tbl, field_name).is_some() {
                            let span = Span::from_node(&c);
                            ctx.emit(Diagnostic::warning(
                                span,
                                Code::TypeMismatch,
                                format!(
                                    "SPLIT field `{}` should be an array type, got `{}`",
                                    field_name, typ
                                ),
                            ));
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// Build the result type from the SELECT field list.
///
/// Receives the `select_clause` node. Its named children are:
/// - `keyword_select`
/// - optionally `keyword_value`
/// - `inclusive_predicate` for each field (or wildcard `*`)
fn build_select_type(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
    is_value: bool,
    omit_fields: &[String],
) -> Kind {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.named_children(&mut cursor).collect();

    // Collect inclusive_predicate nodes (the actual field expressions)
    let predicates: Vec<&Node> = children
        .iter()
        .filter(|c| c.kind() == "inclusive_predicate" || c.kind() == "predicate")
        .collect();

    if predicates.is_empty() {
        return Kind::Any;
    }

    // Check for wildcard SELECT *
    // An inclusive_predicate is a wildcard when it has no `predicate` child
    // (it contains only the unnamed `*` token)
    let is_wildcard = predicates.iter().any(|p| {
        let text = node_text(p, source).trim();
        text == "*"
    });

    if is_wildcard {
        if let Some(tbl) = table {
            if !omit_fields.is_empty() {
                return ctx
                    .build_table_type_with_omit(tbl, omit_fields)
                    .unwrap_or(Kind::Any);
            }
            return ctx.build_table_type(tbl).unwrap_or(Kind::Any);
        }
        return Kind::Any;
    }

    // SELECT VALUE — single field, return its type directly
    if is_value {
        if let Some(pred) = predicates.first() {
            let typ = resolve_expr(pred, source, ctx, table);
            if let Some(top_ident) = find_top_level_field_ident(pred) {
                validate_field_on_table(&top_ident, source, ctx, table);
            }
            return typ;
        }
        return Kind::Any;
    }

    // Multiple fields — build object type
    let mut fields = BTreeMap::new();
    let mut seen_names = HashSet::new();
    for pred in &predicates {
        // Resolve the expression part (before AS alias if present)
        let typ = resolve_predicate_expr(pred, source, ctx, table);

        // Only validate top-level field identifiers — those that are direct
        // field references on the FROM table. Skip any identifiers nested inside
        // paths, subscripts, destructures, graph edges, function calls, etc.
        if let Some(top_ident) = find_top_level_field_ident(pred) {
            validate_field_on_table(&top_ident, source, ctx, table);
        }

        let field_name = extract_field_name(pred, source);
        if let Some(name) = field_name {
            if !seen_names.insert(name.clone()) {
                let span = Span::from_node(pred);
                ctx.emit(Diagnostic::warning(
                    span,
                    Code::DuplicateFieldDef,
                    format!("duplicate field name `{}` in SELECT", name),
                ));
            }
            if !omit_fields.iter().any(|o| o == &name) {
                fields.insert(name, typ);
            }
        }
    }

    if fields.is_empty() {
        Kind::Any
    } else {
        Kind::Literal(Literal::Object(fields))
    }
}

/// Resolve the expression part of a predicate, skipping AS alias.
///
/// For `name AS username`, resolves only `name`.
/// For `name`, resolves `name`.
fn resolve_predicate_expr(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) -> Kind {
    // If this is an inclusive_predicate, look inside for the predicate
    let pred = if node.kind() == "inclusive_predicate" {
        child_by_kind(node, "predicate").unwrap_or(*node)
    } else {
        *node
    };

    // If the predicate has keyword_as, resolve only the value before it
    let has_alias = find_all(&pred, "keyword_as").len() > 0;
    if has_alias {
        // Find the first value child (the expression, not the alias identifier)
        let mut cursor = pred.walk();
        for child in pred.named_children(&mut cursor) {
            if child.kind() == "value" || child.kind() == "base_value" {
                return resolve_expr(&child, source, ctx, table);
            }
        }
    }

    resolve_expr(node, source, ctx, table)
}

/// Extract the output field name from a select predicate expression.
///
/// Handles aliases (name AS alias) and plain field references.
/// Tree structure:
///   inclusive_predicate > predicate > value + keyword_as + identifier(alias)
///   inclusive_predicate > predicate > value > identifier(field_name)
fn extract_field_name(node: &Node, source: &str) -> Option<String> {
    // Look for AS alias pattern: the identifier AFTER keyword_as
    let as_keywords = find_all(node, "keyword_as");
    if !as_keywords.is_empty() {
        // Find the identifier that comes after keyword_as
        // In the predicate children: value, keyword_as, identifier
        let predicates = find_all(node, "predicate");
        for pred in &predicates {
            let mut cursor = pred.walk();
            let mut found_as = false;
            for child in pred.named_children(&mut cursor) {
                if child.kind() == "keyword_as" {
                    found_as = true;
                } else if found_as && child.kind() == "identifier" {
                    return Some(node_text(&child, source).to_string());
                }
            }
        }
        // Fallback: search all identifiers, take the last one (the alias)
        let idents = find_all(node, "identifier");
        if idents.len() >= 2 {
            return Some(node_text(idents.last().unwrap(), source).to_string());
        }
    }

    // No alias — use the first identifier as the field name
    let idents = find_all(node, "identifier");
    if let Some(first_ident) = idents.first() {
        return Some(node_text(first_ident, source).to_string());
    }

    // Fallback: use the node text
    let text = node_text(node, source).trim().to_string();
    if !text.is_empty() {
        Some(text)
    } else {
        None
    }
}

/// Check if this SELECT has the VALUE keyword (inside select_clause).
fn has_value_keyword(node: &Node) -> bool {
    !find_all(node, "keyword_value").is_empty()
}

/// Extract field names from OMIT clause.
fn extract_omit_fields(node: &Node, source: &str) -> Vec<String> {
    let mut fields = Vec::new();
    if let Some(omit) = child_by_kind(node, "omit_clause") {
        for ident in find_all(&omit, "identifier") {
            fields.push(node_text(&ident, source).to_string());
        }
    }
    fields
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

/// Find the top-level field identifier in a predicate — the first identifier
/// that represents a direct field reference on the FROM table.
///
/// For `name` → returns `name`
/// For `author.name` → returns `author` (name is on author, not the table)
/// For `->wrote->post` → returns None (these are all table refs)
/// For `string::len(name)` → returns None (function call, not field)
/// For `author.{id, name}` → returns `author` (destructure fields are on author)
fn find_top_level_field_ident<'a>(pred: &Node<'a>) -> Option<Node<'a>> {
    // Walk down through transparent wrappers to find the first meaningful node
    let mut current = *pred;
    loop {
        let mut cursor = current.walk();
        let children: Vec<_> = current.named_children(&mut cursor).collect();

        match current.kind() {
            // Transparent wrappers — descend into first child
            "predicate" | "value" | "base_value" | "expression"
            | "inclusive_predicate" => {
                if let Some(child) = children.first() {
                    current = *child;
                    continue;
                }
                return None;
            }
            // Direct identifier — this is what we want
            "identifier" => return Some(current),
            // Path expression — only validate the FIRST identifier (the root field)
            "path" => {
                if let Some(first) = children.first() {
                    if first.kind() == "base_value" || first.kind() == "identifier" {
                        // Find the identifier inside base_value
                        let mut c2 = first.walk();
                        for inner in first.named_children(&mut c2) {
                            if inner.kind() == "identifier" {
                                return Some(inner);
                            }
                        }
                        if first.kind() == "identifier" {
                            return Some(*first);
                        }
                    }
                    // If path starts with graph_path (->), no top-level field
                    if first.kind() == "graph_path" {
                        return None;
                    }
                }
                return None;
            }
            // Everything else (function calls, graph paths, etc.) — no field to validate
            _ => return None,
        }
    }
}

/// Check if an identifier node is inside a graph path (->relation->target).
/// These identifiers are table references, not field references.
fn is_inside_graph_path(node: &Node) -> bool {
    let mut current = *node;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            "graph_path" | "graph_predicate" | "graph_expression" => return true,
            // Stop at statement level
            k if k.ends_with("_statement") || k == "select_clause" => return false,
            _ => {}
        }
        current = parent;
    }
    false
}

/// Check if an identifier is inside a subscript (e.g., `.name` in `author.name`).
fn is_inside_subscript(node: &Node) -> bool {
    let mut current = *node;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            "subscript" => return true,
            k if k.ends_with("_statement") || k == "select_clause" || k == "predicate" => return false,
            _ => {}
        }
        current = parent;
    }
    false
}

/// Check if an identifier is inside a function call (e.g., `len` in `string::len()`).
fn is_inside_function_call(node: &Node) -> bool {
    let mut current = *node;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            "function_call" | "builtin_function_name" | "function_name" | "custom_function_name" => return true,
            k if k.ends_with("_statement") || k == "select_clause" || k == "predicate" => return false,
            _ => {}
        }
        current = parent;
    }
    false
}

fn validate_field_on_table(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    if let Some(tbl) = table {
        let field_name = node_text(node, source);
        // Validate on schemafull tables regardless of strict mode —
        // schemafull tables explicitly define their fields, so accessing
        // an undefined field is always a bug.
        if let Some(table_def) = ctx.get_table(tbl) {
            if table_def.schema_mode == crate::context::SchemaMode::Schemafull
                && ctx.get_field(tbl, field_name).is_none()
                && field_name != "id"
                && field_name != "*"
            {
                let span = Span::from_node(node);
                let table_span = table_def.span;
                ctx.emit(Diagnostic::warning(
                    span,
                    Code::UndefinedFieldOnSchemafull,
                    format!(
                        "field `{}` is not defined on table `{}`",
                        field_name, tbl
                    ),
                )
                .with_suggestion(format!("did you mean one of the defined fields? check DEFINE FIELD statements on `{}`", tbl)));
            }
        }
    }
}

/// Validate that identifier fields referenced in a clause (ORDER BY, GROUP BY)
/// exist on schemafull tables.
fn validate_clause_fields(
    node: &Node,
    clause_name: &str,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
) {
    if let Some(tbl) = table {
        if let Some(table_def) = ctx.get_table(tbl) {
            if table_def.schema_mode == crate::context::SchemaMode::Schemafull {
                for ident in find_all(node, "identifier") {
                    let field_name = node_text(&ident, source);
                    if ctx.get_field(tbl, field_name).is_none() && field_name != "id" {
                        let span = Span::from_node(&ident);
                        ctx.emit(Diagnostic::warning(
                            span,
                            Code::FieldNotFound,
                            format!(
                                "field `{}` in {} clause not defined on table `{}`",
                                field_name, clause_name, tbl
                            ),
                        ));
                    }
                }
            }
        }
    }
}

/// Check if a node tree contains any aggregate function calls.
///
/// Aggregate functions include: count, sum, avg, min, max, math::mean,
/// math::median, math::mode, math::spread, math::stddev, math::variance,
/// array::group, array::distinct, array::flatten, array::count.
fn contains_aggregate_function(node: &Node, source: &str) -> bool {
    let fn_nodes = find_all(node, "function_call");
    let fn_nodes2 = find_all(node, "function");
    for fn_node in fn_nodes.iter().chain(fn_nodes2.iter()) {
        let fn_text = node_text(fn_node, source).trim();
        let fn_name = fn_text.split('(').next().unwrap_or("").trim();
        if is_aggregate_function(fn_name) {
            return true;
        }
    }
    false
}

/// Returns true if the given function name is a known aggregate function.
fn is_aggregate_function(name: &str) -> bool {
    matches!(
        name,
        "count"
            | "sum"
            | "avg"
            | "min"
            | "max"
            | "math::mean"
            | "math::median"
            | "math::mode"
            | "math::spread"
            | "math::stddev"
            | "math::variance"
            | "math::sum"
            | "math::max"
            | "math::min"
            | "array::group"
            | "array::distinct"
            | "array::flatten"
            | "array::count"
    )
}

/// Validate that a GROUP BY clause is accompanied by aggregate functions in SELECT.
fn validate_group_by_has_aggregates(
    select_node: &Node,
    group_node: &Node,
    source: &str,
    ctx: &mut Context,
) {
    // Find the select fields node
    let select_fields = child_by_kind(select_node, "select_clause")
        .or_else(|| child_by_kind(select_node, "field_list"))
        .or_else(|| child_by_kind(select_node, "select_fields"));

    if let Some(fields_node) = select_fields {
        // Check for wildcard — wildcard with GROUP BY is suspicious but
        // we only warn if no aggregates are found
        if !contains_aggregate_function(&fields_node, source) {
            let span = Span::from_node(group_node);
            ctx.emit(Diagnostic::warning(
                span,
                Code::GroupByWithoutAggregate,
                "GROUP BY without aggregate functions (count, sum, avg, etc.) in SELECT fields"
                    .to_string(),
            ));
        }
    }
}

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

#[cfg(test)]
mod tests {
    use crate::analyze;
    use crate::diagnostic::Severity;

    #[test]
    fn valid_select_fields() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            SELECT name, age FROM user;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
    }

    #[test]
    fn select_wildcard() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            SELECT * FROM user;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
    }

    #[test]
    fn strict_undefined_field() {
        let result = crate::analyze_strict(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            SELECT nonexistent FROM user;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("nonexistent"))
            .collect();
        assert!(!errors.is_empty(), "Expected error for undefined field");
    }

    #[test]
    fn strict_undefined_table() {
        let result = crate::analyze_strict(
            r#"
            SELECT * FROM nonexistent;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("nonexistent"))
            .collect();
        assert!(!errors.is_empty(), "Expected error for undefined table");
    }

    #[test]
    fn select_with_where() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            SELECT name FROM user WHERE age > 18;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
    }

    #[test]
    fn select_result_type_wildcard() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            SELECT * FROM user;
            "#,
        )
        .unwrap();

        // The context should have the table type built
        let table_type = result.context.build_table_type("user").unwrap();
        if let crate::types::Kind::Literal(crate::types::Literal::Object(fields)) = table_type {
            assert_eq!(fields.len(), 2);
            assert_eq!(fields["name"], crate::types::Kind::String);
            assert_eq!(fields["age"], crate::types::Kind::Int);
        } else {
            panic!("Expected object type");
        }
    }

    #[test]
    fn select_omit_removes_fields() {
        // Test that OMIT is recognized and doesn't cause errors
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            DEFINE FIELD secret ON user TYPE string;
            SELECT * OMIT secret FROM user;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);

        // Verify OMIT works: build type with omit
        let omitted = result
            .context
            .build_table_type_with_omit("user", &["secret".to_string()])
            .unwrap();
        if let crate::types::Kind::Literal(crate::types::Literal::Object(fields)) = omitted {
            assert_eq!(fields.len(), 2, "Expected 2 fields after OMIT, got {:?}", fields);
            assert!(fields.contains_key("name"));
            assert!(fields.contains_key("age"));
            assert!(!fields.contains_key("secret"));
        } else {
            panic!("Expected object type");
        }
    }

    #[test]
    fn select_fetch_expands_records() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD org ON user TYPE record<organization>;
            DEFINE TABLE organization SCHEMAFULL;
            DEFINE FIELD title ON organization TYPE string;
            SELECT name, org FROM user FETCH org;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);

        // Verify FETCH expansion works at the context level
        let org_type = result
            .context
            .build_table_type("organization")
            .unwrap();
        if let crate::types::Kind::Literal(crate::types::Literal::Object(fields)) = org_type {
            assert_eq!(fields["title"], crate::types::Kind::String);
        } else {
            panic!("Expected object type for organization");
        }
    }

    // ── New validation tests ────────────────────────────────────

    #[test]
    fn omit_nonexistent_field_warns() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            SELECT * OMIT nonexistent FROM user;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.severity == Severity::Warning
                    && d.message.contains("OMIT clause not defined on table")
                    && d.message.contains("nonexistent")
            })
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected warning for nonexistent OMIT field, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn omit_existing_field_no_warning() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            SELECT * OMIT age FROM user;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("OMIT clause not defined"))
            .collect();
        assert!(
            warnings.is_empty(),
            "Should not warn for existing OMIT field: {:?}",
            warnings
        );
    }

    #[test]
    fn fetch_non_record_field_warns() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            SELECT name, age FROM user FETCH age;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.severity == Severity::Warning
                    && d.message.contains("FETCH field")
                    && d.message.contains("not a record type")
            })
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected warning for non-record FETCH field, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn fetch_record_field_no_warning() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD org ON user TYPE record<organization>;
            DEFINE TABLE organization SCHEMAFULL;
            DEFINE FIELD title ON organization TYPE string;
            SELECT name, org FROM user FETCH org;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("not a record type"))
            .collect();
        assert!(
            warnings.is_empty(),
            "Should not warn for record FETCH field: {:?}",
            warnings
        );
    }

    #[test]
    fn limit_non_numeric_errors() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            SELECT name FROM user LIMIT "five";
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("LIMIT must be a numeric value"))
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected error for non-numeric LIMIT, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn limit_numeric_no_error() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            SELECT name FROM user LIMIT 10;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("LIMIT"))
            .collect();
        assert!(
            errors.is_empty(),
            "Should not error for numeric LIMIT: {:?}",
            errors
        );
    }

    #[test]
    fn start_non_numeric_errors() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            SELECT name FROM user START "five";
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("START must be a numeric value"))
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected error for non-numeric START, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn timeout_non_duration_errors() {
        // Tree-sitter grammar enforces duration syntax for TIMEOUT,
        // so a non-duration value produces a parse error at the grammar level
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            SELECT name FROM user TIMEOUT 42;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected error for non-duration TIMEOUT, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn timeout_duration_no_error() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            SELECT name FROM user TIMEOUT 5s;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("TIMEOUT"))
            .collect();
        assert!(
            errors.is_empty(),
            "Should not error for duration TIMEOUT: {:?}",
            errors
        );
    }

    #[test]
    fn split_non_array_field_warns() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD tags ON user TYPE array<string>;
            SELECT * FROM user SPLIT name;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.severity == Severity::Warning
                    && d.message.contains("SPLIT field")
                    && d.message.contains("should be an array type")
            })
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected warning for non-array SPLIT field, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn split_array_field_no_warning() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD tags ON user TYPE array<string>;
            SELECT * FROM user SPLIT tags;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("SPLIT field") && d.message.contains("should be an array"))
            .collect();
        assert!(
            warnings.is_empty(),
            "Should not warn for array SPLIT field: {:?}",
            warnings
        );
    }

    #[test]
    fn duplicate_field_name_warns() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            SELECT name, name FROM user;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.severity == Severity::Warning
                    && d.message.contains("duplicate field name")
                    && d.message.contains("name")
            })
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected warning for duplicate field in SELECT, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn no_duplicate_field_no_warning() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            SELECT name, age FROM user;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("duplicate field name"))
            .collect();
        assert!(
            warnings.is_empty(),
            "Should not warn when no duplicates: {:?}",
            warnings
        );
    }

    // ── ORDER BY field existence tests ──────────────────────────

    #[test]
    fn order_by_field_exists_no_error() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            SELECT * FROM user ORDER BY name;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
    }

    #[test]
    fn order_by_field_not_exists_warns() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            SELECT * FROM user ORDER BY nonexistent;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("nonexistent"))
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected warning for nonexistent ORDER BY field, got: {:?}",
            result.diagnostics
        );
    }

    // ── GROUP BY field existence tests ──────────────────────────

    #[test]
    fn group_by_field_exists_no_error() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            SELECT name, count() FROM user GROUP BY name;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
    }

    #[test]
    fn group_by_field_not_exists_warns() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            SELECT name, count() FROM user GROUP BY nonexistent;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("nonexistent"))
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected warning for nonexistent GROUP BY field, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn order_by_id_no_warning() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            SELECT * FROM user ORDER BY id;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("ORDER BY") && d.message.contains("not defined"))
            .collect();
        assert!(
            warnings.is_empty(),
            "Should not warn for id field: {:?}",
            warnings
        );
    }

    // ── SPLIT field existence on schemafull tables ────────────────

    #[test]
    fn split_nonexistent_field_warns() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD tags ON user TYPE array<string>;
            SELECT * FROM user SPLIT nonexistent;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.severity == Severity::Warning
                    && d.message.contains("SPLIT clause not defined on table")
                    && d.message.contains("nonexistent")
            })
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected warning for nonexistent SPLIT field, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn split_existing_field_no_existence_warning() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD tags ON user TYPE array<string>;
            SELECT * FROM user SPLIT tags;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("SPLIT clause not defined"))
            .collect();
        assert!(
            warnings.is_empty(),
            "Should not warn for existing SPLIT field: {:?}",
            warnings
        );
    }

    #[test]
    fn split_nonexistent_field_schemaless_no_warning() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMALESS;
            SELECT * FROM user SPLIT nonexistent;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("SPLIT clause not defined"))
            .collect();
        assert!(
            warnings.is_empty(),
            "Should not warn for schemaless tables: {:?}",
            warnings
        );
    }

    #[test]
    fn split_id_no_warning() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            SELECT * FROM user SPLIT id;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("SPLIT clause not defined"))
            .collect();
        assert!(
            warnings.is_empty(),
            "Should not warn for id field: {:?}",
            warnings
        );
    }

    // ── GROUP BY without aggregate functions ─────────────────────

    #[test]
    fn group_by_with_count_no_warning() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            SELECT name, count() FROM user GROUP BY name;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("GROUP BY without aggregate"))
            .collect();
        assert!(
            warnings.is_empty(),
            "Should not warn when aggregate function is present: {:?}",
            warnings
        );
    }

    #[test]
    fn group_by_without_aggregate_warns() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            SELECT name, age FROM user GROUP BY name;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.severity == Severity::Warning
                    && d.message.contains("GROUP BY without aggregate")
            })
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected warning for GROUP BY without aggregates, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn group_by_with_math_mean_no_warning() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            SELECT name, math::mean(age) FROM user GROUP BY name;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("GROUP BY without aggregate"))
            .collect();
        assert!(
            warnings.is_empty(),
            "Should not warn when math::mean is present: {:?}",
            warnings
        );
    }

    #[test]
    fn group_by_with_array_group_no_warning() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD tags ON user TYPE array<string>;
            SELECT name, array::group(tags) FROM user GROUP BY name;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("GROUP BY without aggregate"))
            .collect();
        assert!(
            warnings.is_empty(),
            "Should not warn when array::group is present: {:?}",
            warnings
        );
    }

    #[test]
    fn no_group_by_no_aggregate_warning() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            SELECT name, age FROM user;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("GROUP BY without aggregate"))
            .collect();
        assert!(
            warnings.is_empty(),
            "Should not warn when there is no GROUP BY: {:?}",
            warnings
        );
    }

    #[test]
    fn select_value_multiple_fields_warns() {
        let result = analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            SELECT VALUE name, age FROM user;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::Code::SelectValueMultipleFields)
            .collect();
        assert!(!warnings.is_empty(), "Expected warning for SELECT VALUE with multiple fields");
    }

    #[test]
    fn select_value_single_field_no_warning() {
        let result = analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            SELECT VALUE name FROM user;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::Code::SelectValueMultipleFields)
            .collect();
        assert!(warnings.is_empty(), "No warning expected for single field SELECT VALUE");
    }

    #[test]
    fn select_with_nonexistent_index_errors() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE INDEX idx_name ON user FIELDS name;
            SELECT * FROM user WITH INDEX nonexistent;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == crate::Code::IndexNotFound)
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected IndexNotFound error for nonexistent index, got: {:?}",
            result.diagnostics
        );
        assert!(errors[0].message.contains("nonexistent"));
        assert!(errors[0].message.contains("user"));
    }

    #[test]
    fn select_with_existing_index_no_error() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE INDEX idx_name ON user FIELDS name;
            SELECT * FROM user WITH INDEX idx_name;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == crate::Code::IndexNotFound)
            .collect();
        assert!(
            errors.is_empty(),
            "Should not error for existing index, got: {:?}",
            errors
        );
    }

}
