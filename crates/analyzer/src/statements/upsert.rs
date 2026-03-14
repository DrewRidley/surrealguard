use tree_sitter::Node;

use crate::context::{Context, SchemaMode};
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{child_by_kind, find_all, node_text};
use crate::resolve::resolve_expr;
use crate::span::Span;

pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> crate::types::Kind {
    let table = super::extract_target_table(node, source);
    let table_ref = table.as_deref();

    if let Some(tbl) = table_ref {
        if ctx.strict && !ctx.has_table(tbl) {
            let span = find_all(node, "identifier")
                .first()
                .map(|id| Span::from_node(id))
                .or_else(|| child_by_kind(node, "upsert_target").map(|t| Span::from_node(&t)))
                .unwrap_or_else(|| Span::from_node(node));
            ctx.emit(Diagnostic::error(
                span,
                Code::TableNotFound,
                format!("table `{}` not defined", tbl),
            ));
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "set_clause" => {
                for assignment in find_all(&child, "field_assignment") {
                    analyze_field_assignment(&assignment, source, ctx, table_ref);
                }
            }
            "content_clause" | "merge_clause" | "replace_clause" => {
                analyze_data_expression(&child, source, ctx, table_ref);
            }
            "patch_clause" => {
                super::validate_patch_syntax(&child, source, ctx);
                analyze_data_expression(&child, source, ctx, table_ref);
            }
            "field_assignment" => {
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
            "where_clause" => {
                let mut inner = child.walk();
                for c in child.named_children(&mut inner) {
                    if c.kind() != "keyword_where" {
                        let typ = resolve_expr(&c, source, ctx, table_ref);
                        if typ != crate::types::Kind::Bool && typ != crate::types::Kind::Any {
                            let span = Span::from_node(&c);
                            ctx.emit(Diagnostic::error(
                                span,
                                Code::TypeMismatch,
                                format!("WHERE clause should be bool, got `{}`", typ),
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Check for duplicate field assignments in SET clauses
    super::check_duplicate_field_assignments(node, source, ctx);

    super::compute_dml_result_type(node, source, ctx, table_ref)
}

fn analyze_data_expression(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    let inner_node = find_data_value(node);
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
            if let Some(tbl) = table {
                if ctx.is_schemafull(tbl) {
                    super::validate_object_against_schema(&inner, source, ctx, tbl);
                } else {
                    resolve_expr(&inner, source, ctx, table);
                }
            } else {
                resolve_expr(&inner, source, ctx, table);
            }
        }
        _ => {
            let resolved = resolve_expr(&inner, source, ctx, table);
            if node.kind() == "content_clause" && crate::types::is_definitely_not_object(&resolved) {
                let span = Span::from_node(&inner);
                ctx.emit(Diagnostic::error(
                    span,
                    Code::ContentNotObject,
                    format!(
                        "CONTENT clause expects an object, but got `{}`",
                        resolved
                    ),
                ));
            }
        }
    }
}

fn find_data_value<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            k if k.starts_with("keyword_") => continue,
            "value" | "base_value" => return find_data_value(&child),
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
    // field_assignment: (identifier | path), assignment_operator, value
    let path_info = super::extract_field_path_info(node, source);
    let mut value_node_opt = None;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
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
        let result = node.children(&mut c).find(|ch| ch.kind() == "identifier" || ch.kind() == "path");
        result
    };

    if let (Some(path_info), Some(value_node)) = (path_info, value_node_opt) {
        let field_name = &path_info.top_level;
        let value_type = resolve_expr(&value_node, source, ctx, table);

        if let Some(tbl) = table {
            let is_schemafull = ctx
                .get_table(tbl)
                .map(|t| t.schema_mode == SchemaMode::Schemafull)
                .unwrap_or(false);
            let field_info = ctx.get_field(tbl, field_name).map(|f| (f.readonly, f.is_computed, f.typ.clone(), f.span));
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

                // Only check type compatibility for simple (non-nested) assignments
                if !path_info.has_subscripts && !path_info.has_array_index {
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

            // Validate nested field path access
            super::validate_nested_field_path(node, ctx, tbl, &path_info);
        }

        // Infer parameter type from field type (only for simple fields)
        if !path_info.has_subscripts && !path_info.has_array_index {
            let inner = unwrap_value(&value_node);
            if inner.kind() == "variable_name" {
                if let Some(tbl) = table {
                    let param_name = node_text(&inner, source);
                    let clean_name = param_name.trim_start_matches('$').to_string();
                    if let Some(field_def) = ctx.get_field(tbl, field_name) {
                        if let Some(field_type) = &field_def.typ {
                            ctx.add_inferred_param(clean_name, field_type.clone());
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::analyze;

    #[test]
    fn upsert_patch_invalid_op_warns() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            UPSERT user PATCH [{"op": "invalid_op", "path": "/name"}];
            "#,
        )
        .unwrap();
        let patch_errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == crate::Code::InvalidPatchOperation)
            .collect();
        assert!(
            !patch_errors.is_empty(),
            "UPSERT with invalid PATCH op should produce a warning, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn upsert_patch_valid_no_error() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            UPSERT user PATCH [{"op": "replace", "path": "/name", "value": "new"}];
            "#,
        )
        .unwrap();
        let patch_errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == crate::Code::InvalidPatchOperation)
            .collect();
        assert!(
            patch_errors.is_empty(),
            "Valid UPSERT PATCH should not produce errors: {:?}",
            patch_errors
        );
    }

    #[test]
    fn upsert_duplicate_field_assignment_warns() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            UPSERT user SET name = 'John', name = 'Jane';
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == crate::Code::DuplicateFieldAssignment)
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected duplicate field assignment warning for UPSERT, got: {:?}",
            result.diagnostics
        );
    }
}
