use tree_sitter::Node;

use crate::context::{Context, SchemaMode};
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{child_by_kind, find_all, node_text};
use crate::resolve::resolve_expr;
use crate::span::Span;

pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> crate::types::Kind {
    let table = super::extract_target_table(node, source);
    let table_ref = table.as_deref();

    // Validate table exists in strict mode
    if let Some(tbl) = table_ref {
        if ctx.strict && !ctx.has_table(tbl) {
            let span = Span::from_node(node);
            ctx.emit(Diagnostic::error(
                span,
                Code::TableNotFound,
                format!("table `{}` not defined", tbl),
            ));
        }
    }

    // Analyze SET/CONTENT clauses with parameter inference
    analyze_data_clause(node, source, ctx, table_ref);

    // Check for duplicate field assignments in SET clauses
    super::check_duplicate_field_assignments(node, source, ctx);

    // Check for missing required fields on schemafull tables
    if let Some(tbl) = table_ref {
        check_missing_required_fields(node, source, ctx, tbl);
    }

    // Warn if RETURN BEFORE is used — CREATE inserts a new record so there is no "before" state
    if let Some(return_clause) = find_all(node, "return_clause").into_iter().next() {
        if child_by_kind(&return_clause, "keyword_before").is_some() {
            let span = Span::from_node(&return_clause);
            ctx.emit(Diagnostic::warning(
                span,
                Code::ReturnBeforeOnCreate,
                "RETURN BEFORE on CREATE is meaningless — there is no previous state for a newly created record".to_string(),
            ));
        }
    }

    super::compute_dml_result_type(node, source, ctx, table_ref)
}

fn analyze_data_clause(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "set_clause" => {
                analyze_set_assignments(&child, source, ctx, table);
            }
            "content_clause" => {
                analyze_content_clause(&child, source, ctx, table);
            }
            "field_assignment" => {
                analyze_field_assignment(&child, source, ctx, table);
            }
            "return_clause" => {
                let mut inner = child.walk();
                for c in child.named_children(&mut inner) {
                    match c.kind() {
                        k if k.starts_with("keyword_") => {}
                        _ => {
                            resolve_expr(&c, source, ctx, table);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn analyze_content_clause(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    // content_clause children: keyword_content, value → base_value → (variable_name | object | ...)
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
            if let Some(tbl) = table {
                if ctx.is_schemafull(tbl) {
                    // validate_object_against_schema resolves each value and checks types
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
            if crate::types::is_definitely_not_object(&resolved) {
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

    // object → object_content → object_property*
    // object_property → object_key, ':', value → base_value → variable_name
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "object_content" {
            infer_from_object_content(&child, source, ctx, tbl);
        } else if child.kind() == "object_property" {
            infer_from_object_property(&child, source, ctx, tbl);
        }
    }
}

fn infer_from_object_content(node: &Node, source: &str, ctx: &mut Context, table: &str) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "object_property" {
            infer_from_object_property(&child, source, ctx, table);
        }
    }
}

fn infer_from_object_property(node: &Node, source: &str, ctx: &mut Context, table: &str) {
    // object_property: object_key, ':', value
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

fn analyze_set_assignments(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    for assignment in find_all(node, "field_assignment") {
        analyze_field_assignment(&assignment, source, ctx, table);
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

    if let (Some(path_info), Some(value_node)) = (path_info, value_node_opt) {
        let field_name = &path_info.top_level;
        let value_type = resolve_expr(&value_node, source, ctx, table);

        if let Some(tbl) = table {
            // Clone field info to avoid borrow conflict with ctx.emit()
            let is_schemafull = ctx
                .get_table(tbl)
                .map(|t| t.schema_mode == SchemaMode::Schemafull)
                .unwrap_or(false);
            let field_info = ctx.get_field(tbl, field_name).map(|f| (f.readonly, f.is_computed, f.typ.clone(), f.span));
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

                // Only check type compatibility for simple (non-nested) assignments
                if !path_info.has_subscripts && !path_info.has_array_index {
                    if let Some(ref expected) = field_type {
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

/// Check that all required fields are provided in a CREATE statement.
fn check_missing_required_fields(node: &Node, source: &str, ctx: &mut Context, table: &str) {
    let required = ctx.required_fields(table);
    if required.is_empty() {
        return;
    }

    let provided = collect_provided_fields(node, source);

    // If we couldn't determine provided fields (e.g. CONTENT $param), skip validation
    let provided = match provided {
        Some(fields) => fields,
        None => return,
    };

    // Point the span at the target (e.g. "user" in "CREATE user SET ...")
    // rather than the entire statement
    let target_span = find_all(node, "create_target")
        .first()
        .map(|n| Span::from_node(n))
        .or_else(|| find_all(node, "identifier").first().map(|n| Span::from_node(n)))
        .unwrap_or_else(|| Span::from_node(node));
    let missing: Vec<&String> = required.iter().filter(|f| !provided.iter().any(|p| p == *f)).collect();
    for field in &missing {
        let mut diag = Diagnostic::warning(
            target_span,
            Code::MissingRequiredField,
            format!(
                "missing required field `{}` in CREATE on table `{}`; field has no DEFAULT and is not computed",
                field, table
            ),
        );
        if let Some(field_def) = ctx.get_field(table, field) {
            let typ_str = field_def.typ.as_ref().map(|t| format!("{}", t)).unwrap_or_else(|| "any".to_string());
            diag = diag.with_related(field_def.span, format!("`{}` defined as `{}` here", field, typ_str));
        }
        ctx.emit(diag);
    }
}

/// Collect field names provided in SET or CONTENT clauses.
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
            "field_assignment" => {
                // Direct field_assignment (some grammars put these directly)
                let mut fields = Vec::new();
                let mut cursor2 = node.walk();
                for c in node.named_children(&mut cursor2) {
                    if c.kind() == "field_assignment" {
                        if let Some(name) = extract_assignment_field_name(&c, source) {
                            fields.push(name);
                        }
                    }
                }
                return Some(fields);
            }
            "content_clause" => {
                let inner = find_content_value(&child);
                match inner.as_ref().map(|n| n.kind()) {
                    Some("variable_name") => return None, // CONTENT $param — can't validate
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
    // No data clause found — no fields provided
    Some(Vec::new())
}

fn extract_assignment_field_name(node: &Node, source: &str) -> Option<String> {
    // Use the shared path info extractor to handle both simple identifiers and paths
    super::extract_field_path_info(node, source).map(|info| info.top_level)
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

#[cfg(test)]
mod tests {
    use crate::analyze;
    use crate::types::Kind;

    #[test]
    fn create_validates_table() {
        let result = crate::analyze_strict("CREATE nonexistent CONTENT { name: 'test' };").unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("nonexistent"))
            .collect();
        assert!(!errors.is_empty(), "Expected error for undefined table");
    }

    #[test]
    fn create_readonly_check() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD id ON user TYPE string READONLY;
            DEFINE FIELD name ON user TYPE string;
            CREATE user SET id = 'test', name = 'John';
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("readonly"))
            .collect();
        assert!(!errors.is_empty(), "Expected readonly error");
    }

    #[test]
    fn create_infer_content_param() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            CREATE user CONTENT $user;
            "#,
        )
        .unwrap();
        let params = result.context.inferred_params();
        assert!(!params.is_empty(), "Should infer parameter type");
    }

    #[test]
    fn create_type_mismatch() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            CREATE user SET name = 'John', age = 'not a number';
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.code == crate::Code::IncompatibleAssignment)
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected type mismatch error for string→int, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn create_compatible_types_no_error() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            CREATE user SET name = 'John', age = 25;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
    }

    #[test]
    fn create_numeric_coercion_allowed() {
        // int should be assignable to number
        let result = analyze(
            r#"
            DEFINE TABLE metric SCHEMAFULL;
            DEFINE FIELD value ON metric TYPE number;
            CREATE metric SET value = 42;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "int→number should be allowed: {:?}", errors);
    }

    #[test]
    fn create_infer_content_fields() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            CREATE user CONTENT { name: $name, age: $age };
            "#,
        )
        .unwrap();
        let params = result.context.inferred_params();
        let has_name = params.iter().any(|(n, t)| n == "name" && *t == Kind::String);
        let has_age = params.iter().any(|(n, t)| n == "age" && *t == Kind::Int);
        assert!(has_name, "Should infer $name as string, got {:?}", params);
        assert!(has_age, "Should infer $age as int, got {:?}", params);
    }

    #[test]
    fn create_computed_field_error() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD display ON user COMPUTED string::uppercase(name);
            CREATE user SET name = 'John', display = 'JOHN';
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("computed"))
            .collect();
        assert!(!errors.is_empty(), "Expected computed field assignment error, got: {:?}", result.diagnostics);
    }

    #[test]
    fn create_missing_required_field_warns() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            CREATE user SET name = 'John';
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
            "Expected missing required field warning for `age`, got: {:?}",
            result.diagnostics
        );
        assert!(
            warnings.iter().any(|w| w.message.contains("age")),
            "Warning should mention `age`, got: {:?}",
            warnings
        );
    }

    #[test]
    fn create_all_required_fields_no_warning() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            CREATE user SET name = 'John', age = 25;
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
            "Should not warn when all required fields are provided: {:?}",
            warnings
        );
    }

    #[test]
    fn create_optional_field_missing_no_warning() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD bio ON user TYPE option<string>;
            CREATE user SET name = 'John';
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
            "Should not warn when optional field is missing: {:?}",
            warnings
        );
    }

    #[test]
    fn create_computed_field_missing_no_warning() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD display ON user COMPUTED string::uppercase(name);
            CREATE user SET name = 'John';
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
            "Should not warn when computed field is missing: {:?}",
            warnings
        );
    }

    #[test]
    fn create_content_param_no_warning() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            CREATE user CONTENT $user;
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
            "Should not warn for CONTENT $param (can't validate): {:?}",
            warnings
        );
    }

    #[test]
    fn create_content_object_missing_field_warns() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            CREATE user CONTENT { name: 'John' };
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
            "Expected missing required field warning for CONTENT object, got: {:?}",
            result.diagnostics
        );
        assert!(
            warnings.iter().any(|w| w.message.contains("age")),
            "Warning should mention `age`, got: {:?}",
            warnings
        );
    }

    #[test]
    fn create_with_return_clause() {
        let _result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            CREATE user SET name = 'John' RETURN name;
            "#,
        )
        .unwrap();
        // Should not crash - just verify no panic
    }

    #[test]
    fn create_duplicate_field_assignment_warns() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            CREATE user SET name = 'John', name = 'Jane';
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
            "Expected duplicate field assignment warning, got: {:?}",
            result.diagnostics
        );
        assert!(
            warnings[0].message.contains("name"),
            "Warning should mention `name`, got: {}",
            warnings[0].message
        );
    }

    #[test]
    fn create_no_duplicate_field_no_warning() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            CREATE user SET name = 'John', age = 25;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == crate::Code::DuplicateFieldAssignment)
            .collect();
        assert!(
            warnings.is_empty(),
            "Should not warn when fields are unique: {:?}",
            warnings
        );
    }

    #[test]
    fn create_default_field_missing_no_warning() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD status ON user TYPE string DEFAULT 'active';
            CREATE user SET name = 'John';
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
            "Should not warn when field with DEFAULT is missing: {:?}",
            warnings
        );
    }

    #[test]
    fn create_return_before_warns() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            CREATE user SET name = 'John' RETURN BEFORE;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.message.contains("BEFORE"))
            .collect();
        assert!(!warnings.is_empty());
    }

    #[test]
    fn create_return_after_no_warning() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            CREATE user SET name = 'John' RETURN AFTER;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.message.contains("BEFORE"))
            .collect();
        assert!(warnings.is_empty());
    }

    #[test]
    fn create_content_string_errors() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            CREATE user CONTENT "not an object";
        "#).unwrap();
        let errors: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::Code::ContentNotObject)
            .collect();
        assert!(!errors.is_empty(), "Expected ContentNotObject error for string literal, got: {:?}", result.diagnostics);
    }

    #[test]
    fn create_content_object_no_error() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            CREATE user CONTENT { name: 'John' };
        "#).unwrap();
        let errors: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::Code::ContentNotObject)
            .collect();
        assert!(errors.is_empty(), "Should not have ContentNotObject error for object literal, got: {:?}", errors);
    }

    #[test]
    fn create_content_int_errors() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            CREATE user CONTENT 42;
        "#).unwrap();
        let errors: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::Code::ContentNotObject)
            .collect();
        assert!(!errors.is_empty(), "Expected ContentNotObject error for int literal, got: {:?}", result.diagnostics);
    }

    #[test]
    fn create_content_param_no_error() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            CREATE user CONTENT $data;
        "#).unwrap();
        let errors: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::Code::ContentNotObject)
            .collect();
        assert!(errors.is_empty(), "Should not flag $param as ContentNotObject (could be any type), got: {:?}", errors);
    }

    #[test]
    fn create_content_object_type_mismatch() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            CREATE user CONTENT { name: 'John', age: 'not a number' };
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.code == crate::Code::IncompatibleAssignment)
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected type mismatch for string→int in CONTENT object, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn create_content_object_compatible_no_error() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            CREATE user CONTENT { name: 'John', age: 25 };
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(
            errors.is_empty(),
            "CONTENT with compatible types should not error: {:?}",
            errors
        );
    }
}
