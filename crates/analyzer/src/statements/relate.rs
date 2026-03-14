use tree_sitter::Node;

use crate::context::{Context, SchemaMode, TableKind};
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{find_all, node_text};
use crate::resolve::resolve_expr;
use crate::span::Span;

pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> crate::types::Kind {
    // RELATE from -> relation_table -> to [SET/CONTENT ...]
    // Tree-sitter wraps each part in relate_subject nodes
    let mut cursor = node.walk();
    let subjects: Vec<Node> = node
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "relate_subject")
        .collect();

    if subjects.len() >= 3 {
        let from_text = extract_table_from_subject(&subjects[0], source);
        let relation_name = extract_table_from_subject(&subjects[1], source);
        let to_text = extract_table_from_subject(&subjects[2], source);

        // Extract table name from record IDs (e.g., user:123 → user)
        let from_table = from_text.split(':').next().unwrap_or(&from_text);
        let to_table = to_text.split(':').next().unwrap_or(&to_text);

        // Clone relation info to avoid borrow conflict with ctx.emit()
        let table_kind = ctx.get_table(&relation_name).map(|t| t.kind.clone());

        match table_kind {
            Some(TableKind::Relation { from, to }) => {
                if let Some(allowed_from) = &from {
                    if !allowed_from.iter().any(|t| t == from_table) {
                        let from_str = allowed_from.join(" | ");
                        let to_str = to.as_ref().map(|t| t.join(" | ")).unwrap_or_else(|| "?".into());
                        let span = Span::from_node(&subjects[0]);
                        let mut diag = Diagnostic::error(
                            span,
                            Code::InvalidRelationTraversal,
                            format!(
                                "relation `{}` connects `{}` -> `{}`, not from `{}`",
                                relation_name, from_str, to_str, from_table
                            ),
                        );
                        if let Some(table_def) = ctx.get_table(&relation_name) {
                            diag = diag.with_related(table_def.span, format!("`{}` defined here", relation_name));
                        }
                        diag = diag.with_suggestion(format!(
                            "allowed source tables for `{}`: {}",
                            relation_name, from_str
                        ));
                        ctx.emit(diag);
                    }
                }
                if let Some(allowed_to) = &to {
                    if !allowed_to.iter().any(|t| t == to_table) {
                        let from_str = from.as_ref().map(|t| t.join(" | ")).unwrap_or_else(|| "?".into());
                        let to_str = allowed_to.join(" | ");
                        let span = Span::from_node(&subjects[2]);
                        let mut diag = Diagnostic::error(
                            span,
                            Code::InvalidRelationTraversal,
                            format!(
                                "relation `{}` connects `{}` -> `{}`, not to `{}`",
                                relation_name, from_str, to_str, to_table
                            ),
                        );
                        if let Some(table_def) = ctx.get_table(&relation_name) {
                            diag = diag.with_related(table_def.span, format!("`{}` defined here", relation_name));
                        }
                        diag = diag.with_suggestion(format!(
                            "allowed target tables for `{}`: {}",
                            relation_name, to_str
                        ));
                        ctx.emit(diag);
                    }
                }
            }
            Some(_) => {
                let span = Span::from_node(&subjects[1]);
                let mut diag = Diagnostic::error(
                    span,
                    Code::NotARelation,
                    format!("table `{}` is not a relation; RELATE requires a relation table", relation_name),
                );
                if let Some(table_def) = ctx.get_table(&relation_name) {
                    diag = diag.with_related(table_def.span, format!("`{}` defined as a normal table here", relation_name));
                }
                diag = diag.with_suggestion(format!(
                    "define `{}` as a relation: `DEFINE TABLE {} TYPE RELATION`",
                    relation_name, relation_name
                ));
                ctx.emit(diag);
            }
            None if ctx.strict => {
                let span = Span::from_node(&subjects[1]);
                ctx.emit(Diagnostic::error(
                    span,
                    Code::TableNotFound,
                    format!("relation table `{}` not defined", relation_name),
                ));
            }
            None => {}
        }
    }

    // Analyze SET/CONTENT clauses
    let relation_table_name = if subjects.len() >= 3 {
        Some(extract_table_from_subject(&subjects[1], source))
    } else {
        None
    };
    let relation_table = relation_table_name.as_deref();
    let mut cursor2 = node.walk();
    for child in node.named_children(&mut cursor2) {
        match child.kind() {
            "set_clause" => {
                for assignment in find_all(&child, "field_assignment") {
                    analyze_field_assignment(&assignment, source, ctx, relation_table);
                }
            }
            "content_clause" => {
                analyze_content_clause(&child, source, ctx, relation_table);
            }
            "return_clause" => {
                let mut inner = child.walk();
                for c in child.named_children(&mut inner) {
                    match c.kind() {
                        k if k.starts_with("keyword_") => {}
                        _ => {
                            resolve_expr(&c, source, ctx, relation_table);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Check for duplicate field assignments in SET clauses
    super::check_duplicate_field_assignments(node, source, ctx);

    super::compute_dml_result_type(node, source, ctx, relation_table)
}

/// Extract table name from a relate_subject node.
/// relate_subject can contain: record_id (object_key:value) or identifier
fn extract_table_from_subject(node: &Node, source: &str) -> String {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "record_id" => {
                // record_id → object_key (table name)
                let mut rc = child.walk();
                for rc_child in child.children(&mut rc) {
                    if rc_child.kind() == "object_key" {
                        return node_text(&rc_child, source).to_string();
                    }
                }
                // Fallback: parse text
                let text = node_text(&child, source);
                return text.split(':').next().unwrap_or(&text).to_string();
            }
            "identifier" => {
                return node_text(&child, source).to_string();
            }
            _ => {}
        }
    }
    node_text(node, source).to_string()
}

fn analyze_content_clause(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    let inner_node = find_content_value(node);
    let inner = match inner_node {
        Some(n) => n,
        None => return,
    };

    match inner.kind() {
        // CONTENT $param → infer param as full table type
        "variable_name" => {
            if let Some(tbl) = table {
                let param_name = node_text(&inner, source);
                let clean_name = param_name.trim_start_matches('$').to_string();
                if let Some(table_type) = ctx.build_table_type(tbl) {
                    ctx.add_inferred_param(clean_name, table_type);
                }
            }
        }
        // CONTENT { field: $param, ... } → infer each param from field type
        "object" => {
            infer_params_from_object(&inner, source, ctx, table);
            resolve_expr(&inner, source, ctx, table);
        }
        _ => {
            resolve_expr(&inner, source, ctx, table);
        }
    }
}

/// Navigate through value/base_value wrappers to find the actual content node.
fn find_content_value<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "keyword_content" | "keyword_merge" | "keyword_patch" | "keyword_replace" => continue,
            "value" | "base_value" => return find_content_value(&child),
            _ => return Some(child),
        }
    }
    None
}

/// Infer parameter types from object literal fields.
fn infer_params_from_object(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    let tbl = match table {
        Some(t) => t,
        None => return,
    };

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "object_content" {
            let mut inner = child.walk();
            for prop in child.named_children(&mut inner) {
                if prop.kind() == "object_property" {
                    infer_from_object_property(&prop, source, ctx, tbl);
                }
            }
        } else if child.kind() == "object_property" {
            infer_from_object_property(&child, source, ctx, tbl);
        }
    }
}

fn infer_from_object_property(node: &Node, source: &str, ctx: &mut Context, table: &str) {
    let mut key_name = None;
    let mut value_node = None;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "object_key" => {
                key_name = Some(node_text(&child, source).to_string());
            }
            "value" | "base_value" => {
                value_node = Some(child);
            }
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

/// Unwrap value/base_value wrappers to get the innermost node.
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
    let mut cursor = node.walk();
    let children: Vec<Node> = node.named_children(&mut cursor).collect();
    if children.len() >= 2 {
        let field_name = node_text(&children[0], source).to_string();

        // `in` and `out` are automatically set by RELATE syntax and are readonly
        if field_name == "in" || field_name == "out" {
            let span = Span::from_node(node);
            ctx.emit(Diagnostic::error(
                span,
                Code::ReadonlyAssignment,
                format!(
                    "cannot assign to field `{}` in RELATE statement — `in` and `out` are automatically set by the RELATE syntax",
                    field_name
                ),
            ));
        }

        let value_type = resolve_expr(&children[children.len() - 1], source, ctx, table);

        if let Some(tbl) = table {
            let is_schemafull = ctx
                .get_table(tbl)
                .map(|t| t.schema_mode == SchemaMode::Schemafull)
                .unwrap_or(false);
            let field_info = ctx.get_field(tbl, &field_name).map(|f| (f.readonly, f.is_computed, f.typ.clone(), f.span));
            if field_info.is_none() && is_schemafull {
                let span = Span::from_node(node);
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
                    let span = Span::from_node(node);
                    ctx.emit(Diagnostic::error(
                        span,
                        Code::ReadonlyAssignment,
                        format!("cannot assign to readonly field `{}`", field_name),
                    )
                    .with_related(field_span, format!("`{}` defined as READONLY here", field_name)));
                }
                if is_computed {
                    let span = Span::from_node(node);
                    ctx.emit(Diagnostic::error(
                        span,
                        Code::ComputedFieldAssignment,
                        format!("cannot assign to computed field `{}`", field_name),
                    )
                    .with_related(field_span, format!("`{}` defined as COMPUTED here", field_name)));
                }

                if let Some(ref expected) = field_type {
                    let operator = super::extract_assignment_operator(node, source);
                    if !crate::types::is_compound_assignable(expected, &value_type, operator) {
                        let span = Span::from_node(node);
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
    }
}

#[cfg(test)]
mod tests {
    use crate::analyze;

    #[test]
    fn relate_not_a_relation() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE TABLE post SCHEMAFULL;
            RELATE user:1 -> post -> user:2;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("not a relation"))
            .collect();
        assert!(!errors.is_empty(), "Expected 'not a relation' error");
    }

    #[test]
    fn relate_duplicate_field_assignment_warns() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE TABLE knows TYPE RELATION IN user OUT user;
            DEFINE FIELD strength ON knows TYPE int;
            RELATE user:1 -> knows -> user:2 SET strength = 5, strength = 10;
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
            "Expected duplicate field assignment warning for RELATE, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn relate_assign_in_out_errors() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE TABLE knows TYPE RELATION IN user OUT user;
            RELATE user:1 -> knows -> user:2 SET in = user:3;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("in"))
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected error for assigning to `in` in RELATE, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn relate_assign_out_errors() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE TABLE knows TYPE RELATION IN user OUT user;
            RELATE user:1 -> knows -> user:2 SET out = user:3;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("out"))
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected error for assigning to `out` in RELATE, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn relate_content_param_inference() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE TABLE knows TYPE RELATION IN user OUT user;
            DEFINE FIELD since ON knows TYPE datetime;
            RELATE user:1 -> knows -> user:2 CONTENT $data;
            "#,
        )
        .unwrap();
        let params = result.context.inferred_params();
        assert!(
            params.iter().any(|(n, _)| n == "data"),
            "Should infer $data type from knows table schema, got: {:?}",
            params
        );
    }

    #[test]
    fn relate_content_object_param_inference() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE TABLE knows TYPE RELATION IN user OUT user;
            DEFINE FIELD since ON knows TYPE datetime;
            RELATE user:1 -> knows -> user:2 CONTENT { since: $when };
            "#,
        )
        .unwrap();
        let params = result.context.inferred_params();
        assert!(
            params.iter().any(|(n, t)| n == "when" && *t == crate::types::Kind::Datetime),
            "Should infer $when as datetime, got: {:?}",
            params
        );
    }

    #[test]
    fn relate_valid_relation() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE TABLE knows TYPE RELATION IN user OUT user;
            RELATE user:1 -> knows -> user:2;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
    }
}
