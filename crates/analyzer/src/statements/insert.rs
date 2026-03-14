use tree_sitter::Node;

use crate::context::{Context, SchemaMode};
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{find_all, node_text};
use crate::resolve::resolve_expr;
use crate::span::Span;

pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> crate::types::Kind {
    // INSERT INTO table ...
    let table = extract_insert_table(node, source);
    let table_ref = table.as_deref();

    if let Some(tbl) = table_ref {
        if ctx.strict && !ctx.has_table(tbl) {
            // Find the table identifier node after keyword_into for precise span
            let span = {
                let mut c = node.walk();
                let mut found_into = false;
                let mut result = None;
                for child in node.children(&mut c) {
                    if child.kind() == "keyword_into" {
                        found_into = true;
                    } else if found_into && child.kind() == "identifier" {
                        result = Some(Span::from_node(&child));
                        break;
                    }
                }
                result.unwrap_or_else(|| Span::from_node(node))
            };
            ctx.emit(Diagnostic::error(
                span,
                Code::TableNotFound,
                format!("table `{}` not defined", tbl),
            ));
        }
    }

    // INSERT RELATION INTO <table> — validate the target table is actually a relation
    let has_relation_keyword = {
        let mut c = node.walk();
        let result = node.children(&mut c).any(|ch| ch.kind() == "keyword_relation");
        result
    };
    if has_relation_keyword {
        if let Some(tbl) = table_ref {
            if let Some(table_def) = ctx.get_table(tbl) {
                if !matches!(table_def.kind, crate::context::TableKind::Relation { .. }) {
                    // Span the table identifier after keyword_into
                    let span = {
                        let mut c = node.walk();
                        let mut found_into = false;
                        let mut result = None;
                        for child in node.children(&mut c) {
                            if child.kind() == "keyword_into" {
                                found_into = true;
                            } else if found_into && child.kind() == "identifier" {
                                result = Some(Span::from_node(&child));
                                break;
                            }
                        }
                        result.unwrap_or_else(|| Span::from_node(node))
                    };
                    let mut diag = Diagnostic::error(
                        span,
                        Code::NotARelation,
                        format!("table `{}` is not a relation; INSERT RELATION requires a relation table", tbl),
                    );
                    diag = diag.with_related(table_def.span, format!("`{}` defined as a normal table here", tbl));
                    diag = diag.with_suggestion(format!(
                        "define `{}` as a relation: `DEFINE TABLE {} TYPE RELATION`",
                        tbl, tbl
                    ));
                    ctx.emit(diag);
                }
            }
        }
    }

    // Analyze value expressions and content clauses
    let mut cursor = node.walk();
    let mut in_on_duplicate = false;
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "content_clause" => {
                analyze_content_clause(&child, source, ctx, table_ref);
            }
            "keyword_values" => {
                // VALUES clause is handled separately via analyze_values_clause
            }
            "set_clause" => {
                for assignment in find_all(&child, "field_assignment") {
                    analyze_field_assignment(&assignment, source, ctx, table_ref);
                }
            }
            "keyword_on_duplicate_key_update" => {
                in_on_duplicate = true;
            }
            "field_assignment" if in_on_duplicate => {
                analyze_field_assignment(&child, source, ctx, table_ref);
            }
            "return_clause" => {
                let mut inner = child.walk();
                for c in child.named_children(&mut inner) {
                    match c.kind() {
                        k if k.starts_with("keyword_") => {}
                        _ => {
                            resolve_expr(&c, source, ctx, table_ref);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Analyze INSERT ... VALUES clause (flat structure under insert_statement)
    analyze_values_clause(node, source, ctx, table_ref);

    // Check for duplicate field assignments in SET clauses
    super::check_duplicate_field_assignments(node, source, ctx);

    // Check for missing required fields on schemafull tables
    if let Some(tbl) = table_ref {
        check_missing_required_fields(node, source, ctx, tbl);
    }

    super::compute_dml_result_type(node, source, ctx, table_ref)
}

/// Check that all required fields are provided in an INSERT statement.
fn check_missing_required_fields(node: &Node, source: &str, ctx: &mut Context, table: &str) {
    let required = ctx.required_fields(table);
    if required.is_empty() {
        return;
    }

    let provided = collect_provided_fields(node, source);
    let provided = match provided {
        Some(fields) => fields,
        None => return,
    };

    // Span the table identifier for missing-field diagnostics
    let span = {
        let mut c = node.walk();
        let mut found_into = false;
        let mut result = None;
        for child in node.children(&mut c) {
            if child.kind() == "keyword_into" {
                found_into = true;
            } else if found_into && child.kind() == "identifier" {
                result = Some(Span::from_node(&child));
                break;
            }
        }
        result.unwrap_or_else(|| Span::from_node(node))
    };
    for field in &required {
        if !provided.iter().any(|p| p == field) {
            ctx.emit(Diagnostic::warning(
                span,
                Code::MissingRequiredField,
                format!(
                    "missing required field `{}` on schemafull table `{}`",
                    field, table
                ),
            ));
        }
    }
}

/// Collect field names provided in SET, CONTENT, or VALUES clauses.
/// Returns None if we can't determine the fields (e.g. CONTENT $param).
fn collect_provided_fields(node: &Node, source: &str) -> Option<Vec<String>> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "set_clause" => {
                let mut fields = Vec::new();
                for assignment in find_all(&child, "field_assignment") {
                    if let Some(name) = extract_assignment_field_name(&assignment, source) {
                        fields.push(name);
                    }
                }
                return Some(fields);
            }
            "content_clause" => {
                let inner = find_content_value(&child);
                match inner.as_ref().map(|n| n.kind()) {
                    Some("variable_name") => return None,
                    Some("object") => {
                        let obj = inner.unwrap();
                        return Some(collect_object_keys(&obj, source));
                    }
                    _ => return None,
                }
            }
            _ => {}
        }
    }

    // Check for VALUES form: column names are identifiers between '(' and ')' before keyword_values
    if let Some((columns, _)) = extract_values_columns_and_tuples(node, source) {
        return Some(columns);
    }

    Some(Vec::new())
}

fn extract_assignment_field_name(node: &Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            return Some(node_text(&child, source).to_string());
        }
    }
    None
}

fn collect_object_keys(node: &Node, source: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "object_content" {
            let mut inner = child.walk();
            for prop in child.named_children(&mut inner) {
                if prop.kind() == "object_property" {
                    if let Some(key) = extract_object_key(&prop, source) {
                        keys.push(key);
                    }
                }
            }
        } else if child.kind() == "object_property" {
            if let Some(key) = extract_object_key(&child, source) {
                keys.push(key);
            }
        }
    }
    keys
}

fn extract_object_key(node: &Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "object_key" {
            return Some(node_text(&child, source).to_string());
        }
    }
    None
}

fn extract_insert_table(node: &Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    let mut found_into = false;
    for child in node.children(&mut cursor) {
        if child.kind() == "keyword_into" {
            found_into = true;
        } else if found_into && child.kind() == "identifier" {
            return Some(node_text(&child, source).to_string());
        }
    }
    super::extract_target_table(node, source)
}

/// Extract column names and value tuples from the INSERT ... VALUES form.
///
/// The tree-sitter grammar produces a flat structure under `insert_statement`:
///   keyword_insert keyword_into identifier '(' identifier ... ')' keyword_values '(' value ... ')' ...
///
/// Column identifiers appear between the first '(' and ')' after the table identifier.
/// Value nodes appear between subsequent '(' and ')' groups after keyword_values.
fn extract_values_columns_and_tuples<'a>(
    node: &Node<'a>,
    source: &'a str,
) -> Option<(Vec<String>, Vec<Vec<Node<'a>>>)> {
    let mut cursor = node.walk();
    let all_children: Vec<Node> = node.children(&mut cursor).collect();

    // Find keyword_values position; if absent, this is not a VALUES form
    let values_pos = all_children
        .iter()
        .position(|c| c.kind() == "keyword_values")?;

    // Find keyword_into position
    let into_pos = all_children
        .iter()
        .position(|c| c.kind() == "keyword_into")?;

    // Collect column names: identifiers after keyword_into, skipping the table (first identifier)
    let mut columns = Vec::new();
    let mut found_table = false;
    for child in &all_children[into_pos + 1..values_pos] {
        if child.kind() == "identifier" {
            if !found_table {
                found_table = true; // skip table name
            } else {
                columns.push(node_text(child, source).to_string());
            }
        }
    }

    if columns.is_empty() {
        return None; // No columns specified
    }

    // Collect value tuples: group value nodes by '(' ... ')' after keyword_values
    let mut tuples: Vec<Vec<Node>> = Vec::new();
    let mut current_tuple: Option<Vec<Node>> = None;

    for child in &all_children[values_pos + 1..] {
        match child.kind() {
            "(" => {
                current_tuple = Some(Vec::new());
            }
            ")" => {
                if let Some(tuple) = current_tuple.take() {
                    tuples.push(tuple);
                }
            }
            "," => {} // skip commas
            "keyword_on_duplicate_key_update" | "field_assignment" => break,
            _ => {
                if let Some(ref mut tuple) = current_tuple {
                    if child.is_named() {
                        tuple.push(*child);
                    }
                }
            }
        }
    }

    Some((columns, tuples))
}

/// Analyze the VALUES clause of an INSERT statement.
///
/// Validates:
/// 1. Column count vs value count in each tuple
/// 2. Column existence on schemafull tables
/// 3. Value type vs column type compatibility
fn analyze_values_clause(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    let (columns, tuples) = match extract_values_columns_and_tuples(node, source) {
        Some(result) => result,
        None => return,
    };

    let col_count = columns.len();

    // Check column existence on schemafull tables
    if let Some(tbl) = table {
        let is_schemafull = ctx
            .get_table(tbl)
            .map(|t| t.schema_mode == SchemaMode::Schemafull)
            .unwrap_or(false);

        if is_schemafull {
            // Find column identifier nodes for precise spans
            let col_ident_nodes = {
                let mut c = node.walk();
                let all: Vec<Node> = node.children(&mut c).collect();
                let values_pos = all.iter().position(|c| c.kind() == "keyword_values");
                let into_pos = all.iter().position(|c| c.kind() == "keyword_into");
                let mut col_nodes = Vec::new();
                if let (Some(into_p), Some(val_p)) = (into_pos, values_pos) {
                    let mut found_table = false;
                    for child in &all[into_p + 1..val_p] {
                        if child.kind() == "identifier" {
                            if !found_table {
                                found_table = true;
                            } else {
                                col_nodes.push(*child);
                            }
                        }
                    }
                }
                col_nodes
            };
            for (i, col_name) in columns.iter().enumerate() {
                if ctx.get_field(tbl, col_name).is_none() {
                    let span = col_ident_nodes.get(i)
                        .map(|n| Span::from_node(n))
                        .unwrap_or_else(|| Span::from_node(node));
                    let table_span = ctx.get_table(tbl).map(|t| t.span);
                    let mut diag = Diagnostic::error(
                        span,
                        Code::UndefinedFieldOnSchemafull,
                        format!(
                            "field `{}` is not defined on table `{}`",
                            col_name, tbl
                        ),
                    )
                    .with_suggestion(format!("consider adding `DEFINE FIELD {} ON {} TYPE ...`", col_name, tbl));
                    if let Some(tbl_span) = table_span {
                        diag = diag.with_related(tbl_span, format!("table `{}` defined as SCHEMAFULL here", tbl));
                    }
                    ctx.emit(diag);
                }
            }
        }
    }

    // Validate each value tuple
    for tuple in &tuples {
        let val_count = tuple.len();

        // Check column count vs value count
        if val_count != col_count {
            let span = if let Some(first) = tuple.first() {
                Span::from_node(first)
            } else {
                Span::from_node(node)
            };
            ctx.emit(Diagnostic::error(
                span,
                Code::ValuesColumnCountMismatch,
                format!(
                    "expected {} value(s) to match {} column(s), but got {}",
                    col_count, col_count, val_count
                ),
            ));
        }

        // Resolve each value expression and check types against columns
        for (i, value_node) in tuple.iter().enumerate() {
            let value_type = resolve_expr(value_node, source, ctx, table);

            // Type check against column definition if we have a matching column
            if let (Some(tbl), Some(col_name)) = (table, columns.get(i)) {
                if let Some(field_def) = ctx.get_field(tbl, col_name) {
                    let field_span = field_def.span;
                    if let Some(ref expected) = field_def.typ {
                        if !crate::types::is_assignable(expected, &value_type) {
                            let span = Span::from_node(value_node);
                            ctx.emit(Diagnostic::error(
                                span,
                                Code::IncompatibleAssignment,
                                format!(
                                    "expected `{}`, found `{}` in assignment to column `{}`",
                                    expected, value_type, col_name
                                ),
                            )
                            .with_related(field_span, format!("`{}` defined as `{}` here", col_name, expected)));
                        }
                    }
                }
            }
        }
    }
}

fn analyze_content_clause(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    let inner_node = find_content_value(node);
    let inner = match inner_node {
        Some(n) => n,
        None => return,
    };

    match inner.kind() {
        "variable_name" => {
            if let Some(tbl) = table {
                let param_name = node_text(&inner, source);
                let clean_name = param_name.trim_start_matches('$').to_string();
                if let Some(table_type) = ctx.build_table_type(tbl) {
                    ctx.add_inferred_param(clean_name, table_type);
                }
            }
        }
        "object" => {
            infer_params_from_object(&inner, source, ctx, table);
            resolve_expr(&inner, source, ctx, table);
        }
        _ => {
            resolve_expr(&inner, source, ctx, table);
        }
    }
}

fn find_content_value<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            k if k.starts_with("keyword_") => continue,
            "value" | "base_value" => return find_content_value(&child),
            _ => return Some(child),
        }
    }
    None
}

fn infer_params_from_object(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    let tbl = match table {
        Some(t) => t,
        None => return,
    };
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "object_content" {
            let mut inner_cursor = child.walk();
            for prop in child.named_children(&mut inner_cursor) {
                if prop.kind() == "object_property" {
                    infer_from_property(&prop, source, ctx, tbl);
                }
            }
        } else if child.kind() == "object_property" {
            infer_from_property(&child, source, ctx, tbl);
        }
    }
}

fn infer_from_property(node: &Node, source: &str, ctx: &mut Context, table: &str) {
    let mut key_name = None;
    let mut value_node = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "object_key" => key_name = Some(node_text(&child, source).to_string()),
            "value" | "base_value" => value_node = Some(child),
            _ => {}
        }
    }
    if let (Some(key), Some(val)) = (key_name, value_node) {
        let inner = unwrap_value(&val);
        if inner.kind() == "variable_name" {
            let param_name = node_text(&inner, source);
            let clean_name = param_name.trim_start_matches('$').to_string();
            if let Some(field_def) = ctx.get_field(table, &key) {
                if let Some(field_type) = &field_def.typ {
                    ctx.add_inferred_param(clean_name, field_type.clone());
                }
            }
        }
    }
}

fn unwrap_value<'a>(node: &Node<'a>) -> Node<'a> {
    match node.kind() {
        "value" | "base_value" => {
            let mut cursor = node.walk();
            if let Some(child) = node.named_children(&mut cursor).next() {
                return unwrap_value(&child);
            }
            *node
        }
        _ => *node,
    }
}

fn analyze_field_assignment(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    let mut field_name_opt = None;
    let mut value_node_opt = None;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" if field_name_opt.is_none() => {
                field_name_opt = Some(node_text(&child, source).to_string());
            }
            "value" | "base_value" => {
                value_node_opt = Some(child);
            }
            _ => {}
        }
    }

    let operator = super::extract_assignment_operator(node, source);

    // Find the field identifier node for precise spans
    let field_ident_node = {
        let mut c = node.walk();
        let result = node.children(&mut c).find(|ch| ch.kind() == "identifier");
        result
    };

    if let (Some(field_name), Some(value_node)) = (field_name_opt, value_node_opt) {
        let value_type = resolve_expr(&value_node, source, ctx, table);

        if let Some(tbl) = table {
            let is_schemafull = ctx
                .get_table(tbl)
                .map(|t| t.schema_mode == SchemaMode::Schemafull)
                .unwrap_or(false);
            let field_info = ctx
                .get_field(tbl, &field_name)
                .map(|f| (f.readonly, f.is_computed, f.typ.clone(), f.span));
            if field_info.is_none() && is_schemafull {
                let span = field_ident_node
                    .map(|n| Span::from_node(&n))
                    .unwrap_or_else(|| Span::from_node(node));
                let table_span = ctx.get_table(tbl).map(|t| t.span);
                let mut diag = Diagnostic::error(
                    span,
                    Code::UndefinedFieldOnSchemafull,
                    format!(
                        "field `{}` is not defined on table `{}`",
                        field_name, tbl
                    ),
                )
                .with_suggestion(format!("consider adding `DEFINE FIELD {} ON {} TYPE ...`", field_name, tbl));
                if let Some(tbl_span) = table_span {
                    diag = diag.with_related(tbl_span, format!("table `{}` defined as SCHEMAFULL here", tbl));
                }
                ctx.emit(diag);
            } else if let Some((is_readonly, is_computed, field_type, field_span)) = field_info {
                if is_readonly {
                    let span = field_ident_node
                        .map(|n| Span::from_node(&n))
                        .unwrap_or_else(|| Span::from_node(node));
                    ctx.emit(Diagnostic::error(
                        span,
                        Code::ReadonlyAssignment,
                        format!("cannot assign to readonly field `{}`", field_name),
                    )
                    .with_related(field_span, format!("`{}` defined as READONLY here", field_name)));
                }
                if is_computed {
                    let span = field_ident_node
                        .map(|n| Span::from_node(&n))
                        .unwrap_or_else(|| Span::from_node(node));
                    ctx.emit(Diagnostic::error(
                        span,
                        Code::ComputedFieldAssignment,
                        format!("cannot assign to computed field `{}`", field_name),
                    )
                    .with_related(field_span, format!("`{}` defined as COMPUTED here", field_name)));
                }

                if let Some(ref expected) = field_type {
                    if !crate::types::is_compound_assignable(expected, &value_type, operator) {
                        let span = Span::from_node(&value_node);
                        ctx.emit(Diagnostic::error(
                            span,
                            Code::IncompatibleAssignment,
                            format!(
                                "expected `{}`, found `{}` in assignment to field `{}` with `{}`",
                                expected, value_type, field_name, operator
                            ),
                        )
                        .with_related(field_span, format!("`{}` defined as `{}` here", field_name, expected)));
                    }
                }
            }
        }

        // Infer parameter type from field type
        let inner = unwrap_value(&value_node);
        if inner.kind() == "variable_name" {
            if let Some(tbl) = table {
                let param_name = node_text(&inner, source);
                let clean_name = param_name.trim_start_matches('$').to_string();
                if let Some(field_def) = ctx.get_field(tbl, &field_name) {
                    if let Some(field_type) = &field_def.typ {
                        ctx.add_inferred_param(clean_name, field_type.clone());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn insert_missing_required_field_warns() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            INSERT INTO user CONTENT { name: 'John' };
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == crate::Code::MissingRequiredField)
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected missing required field warning for INSERT, got: {:?}",
            result.diagnostics
        );
        assert!(
            warnings.iter().any(|w| w.message.contains("age")),
            "Warning should mention `age`, got: {:?}",
            warnings
        );
    }

    #[test]
    fn insert_content_param_no_warning() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            INSERT INTO user CONTENT $user;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == crate::Code::MissingRequiredField)
            .collect();
        assert!(
            warnings.is_empty(),
            "Should not warn for INSERT CONTENT $param: {:?}",
            warnings
        );
    }

    #[test]
    fn insert_validates_table_strict() {
        let result = crate::analyze_strict("INSERT INTO nonexistent { name: 'test' };").unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("nonexistent"))
            .collect();
        assert!(!errors.is_empty(), "Expected error for undefined table");
    }

    #[test]
    fn insert_values_column_count_mismatch() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            INSERT INTO user (name, age) VALUES ('John', 25, 'extra');
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == crate::Code::ValuesColumnCountMismatch)
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected column count mismatch error, got: {:?}",
            result.diagnostics
        );
        assert!(
            errors[0].message.contains("expected 2") && errors[0].message.contains("got 3"),
            "Error should mention expected 2 and got 3, got: {:?}",
            errors[0].message
        );
    }

    #[test]
    fn insert_values_column_count_mismatch_too_few() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            INSERT INTO user (name, age) VALUES ('John');
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == crate::Code::ValuesColumnCountMismatch)
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected column count mismatch error for too few values, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn insert_values_column_existence() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            INSERT INTO user (name, nonexistent) VALUES ('John', 'val');
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.code == crate::Code::UndefinedFieldOnSchemafull
                    && d.message.contains("nonexistent")
            })
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected undefined field error for nonexistent column, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn insert_values_type_mismatch() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            INSERT INTO user (name, age) VALUES (42, 'not_a_number');
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == crate::Code::IncompatibleAssignment)
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected type mismatch errors for VALUES, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn insert_values_valid_no_errors() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            INSERT INTO user (name, age) VALUES ('John', 25), ('Jane', 30);
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.code == crate::Code::ValuesColumnCountMismatch
                    || d.code == crate::Code::UndefinedFieldOnSchemafull
                    || d.code == crate::Code::IncompatibleAssignment
            })
            .collect();
        assert!(
            errors.is_empty(),
            "Expected no VALUES-related errors for valid INSERT, got: {:?}",
            errors
        );
    }

    #[test]
    fn insert_relation_not_relation_errors() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            INSERT RELATION INTO user { in: user:1, out: user:2 };
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("not a relation"))
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected 'not a relation' error, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn insert_relation_valid_no_error() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE TABLE knows TYPE RELATION FROM user TO user;
            INSERT RELATION INTO knows { in: user:1, out: user:2 };
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("not a relation"))
            .collect();
        assert!(
            errors.is_empty(),
            "Expected no 'not a relation' errors, got: {:?}",
            errors
        );
    }

    #[test]
    fn insert_on_duplicate_key_field_existence() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            INSERT INTO user (name, age) VALUES ('John', 25)
                ON DUPLICATE KEY UPDATE nonexistent = 'val';
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.code == crate::Code::UndefinedFieldOnSchemafull
                    && d.message.contains("nonexistent")
            })
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected undefined field error in ON DUPLICATE KEY UPDATE, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn insert_on_duplicate_key_type_mismatch() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            INSERT INTO user (name, age) VALUES ('John', 25)
                ON DUPLICATE KEY UPDATE age = 'not_a_number';
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.code == crate::Code::IncompatibleAssignment
                    && d.message.contains("age")
            })
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected type mismatch error in ON DUPLICATE KEY UPDATE, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn insert_on_duplicate_key_valid_no_error() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            INSERT INTO user (name, age) VALUES ('John', 25)
                ON DUPLICATE KEY UPDATE age = 30;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| {
                d.code == crate::Code::UndefinedFieldOnSchemafull
                    || d.code == crate::Code::IncompatibleAssignment
            })
            .collect();
        assert!(
            errors.is_empty(),
            "Expected no errors for valid ON DUPLICATE KEY UPDATE, got: {:?}",
            errors
        );
    }
}
