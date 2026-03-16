use tree_sitter::Node;

use crate::context::Context;
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{child_by_kind, node_text};
use crate::resolve::resolve_expr;
use crate::scope::{Binding, BindingKind, ScopeKind};
use crate::span::Span;
use crate::types::Kind;

pub fn analyze(node: &Node, source: &str, ctx: &mut Context) {
    match node.kind() {
        "define_event_statement" => analyze_event(node, source, ctx),
        "define_param_statement" => analyze_param(node, source, ctx),
        "define_function_statement" => analyze_function(node, source, ctx),
        "define_field_statement" => analyze_field(node, source, ctx),
        "define_table_statement" => analyze_table(node, source, ctx),
        "define_scope_statement" => analyze_scope(node, source, ctx),
        _ => {
            // Other DEFINE statements are handled by schema extraction
        }
    }
}

fn analyze_event(node: &Node, source: &str, ctx: &mut Context) {
    // DEFINE EVENT ... ON table WHEN ... THEN ...
    // Set up event scope with $event, $before, $after, $value, $input

    let table_name = if let Some(on_clause) = child_by_kind(node, "on_table_clause") {
        child_by_kind(&on_clause, "identifier")
            .map(|n| node_text(&n, source).to_string())
    } else {
        None
    };

    let table_type = table_name
        .as_deref()
        .and_then(|t| ctx.build_table_type(t))
        .unwrap_or(Kind::Any);

    // Analyze WHEN clause
    if let Some(when_clause) = child_by_kind(node, "when_clause") {
        ctx.scope.push(ScopeKind::Event);
        ctx.scope
            .set_event_scope(table_type.clone(), Span::from_node(node));

        let mut cursor = when_clause.walk();
        for child in when_clause.named_children(&mut cursor) {
            if child.kind() != "keyword_when" {
                resolve_expr(&child, source, ctx, table_name.as_deref());
            }
        }
        ctx.scope.pop();
    }

    // Analyze THEN clause
    if let Some(then_clause) = child_by_kind(node, "then_clause") {
        ctx.scope.push(ScopeKind::Event);
        ctx.scope
            .set_event_scope(table_type, Span::from_node(node));

        let mut cursor = then_clause.walk();
        for child in then_clause.named_children(&mut cursor) {
            if child.kind() != "keyword_then" {
                resolve_expr(&child, source, ctx, table_name.as_deref());
            }
        }
        ctx.scope.pop();
    }
}

fn analyze_param(node: &Node, source: &str, ctx: &mut Context) {
    // DEFINE PARAM $name VALUE expr
    let mut cursor = node.walk();
    let mut param_name = None;
    let mut param_span = Span::new(0, 0);

    for child in node.named_children(&mut cursor) {
        if child.kind() == "variable" || child.kind() == "parameter" || child.kind() == "variable_name" {
            let name = node_text(&child, source);
            param_name = Some(if name.starts_with('$') {
                name.to_string()
            } else {
                format!("${}", name)
            });
            param_span = Span::from_node(&child);
        }
    }

    if let Some(name) = param_name {
        // Type comes from TYPE clause if present, otherwise infer from VALUE expression
        let typ = child_by_kind(node, "type_clause")
            .and_then(|tc| child_by_kind(&tc, "type"))
            .and_then(|t| crate::schema::parse_type(&t, source))
            .or_else(|| {
                // Infer type from VALUE expression
                let mut cursor2 = node.walk();
                let children: Vec<_> = node.named_children(&mut cursor2).collect();
                children.iter()
                    .find(|c| {
                        c.kind() != "keyword_define"
                            && c.kind() != "keyword_param"
                            && c.kind() != "variable_name"
                            && c.kind() != "variable"
                            && c.kind() != "parameter"
                            && c.kind() != "type_clause"
                            && !c.kind().starts_with("keyword_")
                    })
                    .map(|val| resolve_expr(val, source, ctx, None))
            })
            .unwrap_or(Kind::Any);

        ctx.scope.bind(Binding {
            name,
            typ,
            span: param_span,
            mutable: false,
            kind: BindingKind::DefineParam,
        });
    }
}

fn analyze_function(node: &Node, source: &str, ctx: &mut Context) {
    // DEFINE FUNCTION fn::name($a: type, $b: type) -> return_type { body }
    ctx.scope.push(ScopeKind::Function);

    // Bind parameters
    if let Some(param_list) = child_by_kind(node, "param_list") {
        let mut cursor = param_list.walk();
        for child in param_list.named_children(&mut cursor) {
            if child.kind() == "param_definition" || child.kind() == "variable" || child.kind() == "variable_name" {
                let name = node_text(&child, source);
                let var_name = if name.starts_with('$') {
                    name.to_string()
                } else {
                    format!("${}", name)
                };

                let typ = child_by_kind(&child, "type")
                    .and_then(|t| crate::schema::parse_type(&t, source))
                    .unwrap_or(Kind::Any);

                ctx.scope.bind(Binding {
                    name: var_name,
                    typ,
                    span: Span::from_node(&child),
                    mutable: false,
                    kind: BindingKind::FunctionParam,
                });
            }
        }
    }

    // Extract declared return type from `-> type` clause
    let declared_return_type = child_by_kind(node, "returns_clause")
        .and_then(|rc| child_by_kind(&rc, "type"))
        .and_then(|t| crate::schema::parse_type(&t, source));

    // Analyze function body and capture the resolved return type
    let body_type = if let Some(body) = child_by_kind(node, "block") {
        resolve_expr(&body, source, ctx, None)
    } else {
        Kind::Null
    };

    // Compare declared return type against actual body return type
    if let Some(ref declared) = declared_return_type {
        if !matches!(declared, Kind::Any) && !matches!(body_type, Kind::Any) {
            if !crate::types::is_assignable(declared, &body_type) {
                let span = child_by_kind(node, "returns_clause")
                    .map(|rc| Span::from_node(&rc))
                    .unwrap_or_else(|| Span::from_node(node));
                ctx.emit(Diagnostic::warning(
                    span,
                    Code::ReturnTypeMismatch,
                    format!(
                        "expected `{}`, found `{}`",
                        declared, body_type
                    ),
                ));
            }
        }

        // Check if all code paths return a value when a return type is declared
        if let Some(body) = child_by_kind(node, "block") {
            let flow = crate::resolve::analyze_flow(&body, source, ctx, None);
            if !flow.always_returns && !matches!(declared, Kind::Null) {
                let fn_name = child_by_kind(node, "custom_function_name")
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_else(|| "function".to_string());
                let span = child_by_kind(node, "returns_clause")
                    .map(|rc| Span::from_node(&rc))
                    .unwrap_or_else(|| Span::from_node(node));
                ctx.emit(Diagnostic::warning(
                    span,
                    Code::ReturnTypeMismatch,
                    format!(
                        "not all code paths in `{}` return a value of type `{}`",
                        fn_name, declared
                    ),
                ));
            }
        }
    }

    ctx.scope.pop();
}

fn analyze_scope(node: &Node, source: &str, ctx: &mut Context) {
    // DEFINE SCOPE name SIGNIN (...) SIGNUP (...)
    // The SIGNIN/SIGNUP clauses contain sub-queries (SELECT/CREATE) that should
    // be analyzed. Scope login parameters like $email, $pass are dynamic, so we
    // bind them as Kind::Any.

    // Analyze SIGNIN clause
    if let Some(signin_clause) = child_by_kind(node, "signin_clause") {
        ctx.scope.push(ScopeKind::Block);

        // Bind common scope authentication parameters as Any
        for param in &["$email", "$pass", "$username", "$password"] {
            ctx.scope.bind(Binding {
                name: param.to_string(),
                typ: Kind::Any,
                span: Span::from_node(node),
                mutable: false,
                kind: BindingKind::Implicit,
            });
        }

        let mut cursor = signin_clause.walk();
        for child in signin_clause.named_children(&mut cursor) {
            if !child.kind().starts_with("keyword_") {
                resolve_expr(&child, source, ctx, None);
            }
        }
        ctx.scope.pop();
    }

    // Analyze SIGNUP clause
    if let Some(signup_clause) = child_by_kind(node, "signup_clause") {
        ctx.scope.push(ScopeKind::Block);

        // Bind common scope authentication parameters as Any
        for param in &["$email", "$pass", "$username", "$password"] {
            ctx.scope.bind(Binding {
                name: param.to_string(),
                typ: Kind::Any,
                span: Span::from_node(node),
                mutable: false,
                kind: BindingKind::Implicit,
            });
        }

        let mut cursor = signup_clause.walk();
        for child in signup_clause.named_children(&mut cursor) {
            if !child.kind().starts_with("keyword_") {
                resolve_expr(&child, source, ctx, None);
            }
        }
        ctx.scope.pop();
    }
}

fn analyze_field(node: &Node, source: &str, ctx: &mut Context) {
    let table_name = child_by_kind(node, "on_table_clause")
        .and_then(|on| child_by_kind(&on, "identifier"))
        .map(|n| node_text(&n, source).to_string());

    let field_type = child_by_kind(node, "type_clause")
        .and_then(|tc| child_by_kind(&tc, "type"))
        .and_then(|t| crate::schema::parse_type(&t, source))
        .unwrap_or(Kind::Any);

    // Check ASSERT clause expression type
    if let Some(assert_clause) = child_by_kind(node, "assert_clause") {
        ctx.scope.push(ScopeKind::Block);

        ctx.scope.bind(Binding {
            name: "$value".to_string(),
            typ: field_type.clone(),
            span: Span::from_node(node),
            mutable: false,
            kind: BindingKind::DefineParam,
        });

        let mut cursor = assert_clause.walk();
        for child in assert_clause.named_children(&mut cursor) {
            let result_type = resolve_expr(&child, source, ctx, table_name.as_deref());
            if !matches!(result_type, Kind::Bool | Kind::Any) {
                ctx.emit(Diagnostic::warning(
                    Span::from_node(&child),
                    Code::AssertNotBool,
                    format!(
                        "ASSERT expression should be boolean, found `{}`",
                        result_type
                    ),
                ));
            }
        }

        ctx.scope.pop();
    }

    // Check DEFAULT clause expression type against field type
    if let Some(default_clause) = child_by_kind(node, "default_clause") {
        ctx.scope.push(ScopeKind::Block);

        ctx.scope.bind(Binding {
            name: "$value".to_string(),
            typ: field_type.clone(),
            span: Span::from_node(node),
            mutable: false,
            kind: BindingKind::DefineParam,
        });

        let mut cursor = default_clause.walk();
        for child in default_clause.named_children(&mut cursor) {
            if child.kind() == "keyword_default" || child.kind() == "keyword_always" {
                continue;
            }
            let expr_type = resolve_expr(&child, source, ctx, table_name.as_deref());
            if !matches!(field_type, Kind::Any) && !matches!(expr_type, Kind::Any) {
                if !crate::types::is_assignable(&field_type, &expr_type) {
                    ctx.emit(Diagnostic::warning(
                        Span::from_node(&child),
                        Code::DefaultTypeMismatch,
                        format!(
                            "expected `{}`, found `{}` in DEFAULT expression",
                            field_type, expr_type
                        ),
                    ));
                }
            }
        }

        ctx.scope.pop();
    }

    // Analyze PERMISSIONS clause
    if let Some(perms) = child_by_kind(node, "permissions_for_clause") {
        crate::permissions::analyze_permission_clause(&perms, source, ctx, table_name.as_deref());
    }

    // Check VALUE clause expression type against field type
    if let Some(value_clause) = child_by_kind(node, "value_clause") {
        ctx.scope.push(ScopeKind::Block);

        ctx.scope.bind(Binding {
            name: "$value".to_string(),
            typ: field_type.clone(),
            span: Span::from_node(node),
            mutable: false,
            kind: BindingKind::DefineParam,
        });

        let mut cursor = value_clause.walk();
        for child in value_clause.named_children(&mut cursor) {
            if child.kind() == "keyword_value" {
                continue;
            }
            let expr_type = resolve_expr(&child, source, ctx, table_name.as_deref());
            if !matches!(field_type, Kind::Any) && !matches!(expr_type, Kind::Any) {
                if !crate::types::is_assignable(&field_type, &expr_type) {
                    ctx.emit(Diagnostic::warning(
                        Span::from_node(&child),
                        Code::ValueTypeMismatch,
                        format!(
                            "expected `{}`, found `{}` in VALUE expression",
                            field_type, expr_type
                        ),
                    ));
                }
            }
        }

        ctx.scope.pop();
    }
}

fn analyze_table(node: &Node, source: &str, ctx: &mut Context) {
    // Extract table name
    let table_name = {
        let mut cursor = node.walk();
        let mut name = None;
        for child in node.children(&mut cursor) {
            if child.kind() == "identifier" {
                name = Some(node_text(&child, source).to_string());
                break;
            }
        }
        name
    };

    // Analyze AS SELECT ... (view table)
    if let Some(view_clause) = child_by_kind(node, "table_view_clause") {
        analyze_table_view(&view_clause, source, ctx);
    }

    // Analyze PERMISSIONS clause
    if let Some(perms) = child_by_kind(node, "permissions_for_clause") {
        crate::permissions::analyze_permission_clause(&perms, source, ctx, table_name.as_deref());
    }

    // CHANGEFEED clause — the grammar enforces a duration argument,
    // so if the node parsed successfully, the duration is present.
    // No additional validation needed beyond what tree-sitter provides.
}

/// Analyze the AS SELECT ... clause of a DEFINE TABLE view.
///
/// The `table_view_clause` grammar is:
///   AS SELECT fields FROM source [WHERE ...] [GROUP BY ...]
///
/// We validate:
/// - The FROM table(s) exist (strict mode)
/// - Field expressions resolve correctly
/// - WHERE clause is boolean
fn analyze_table_view(node: &Node, source: &str, ctx: &mut Context) {
    // Extract the source table from the FROM part of the view clause.
    // The grammar has: keyword_as, keyword_select, inclusive_predicate+, keyword_from, value+, ...
    let from_table = extract_view_from_table(node, source);
    let table_ref = from_table.as_deref();

    // Validate FROM table exists (strict mode)
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

    // Walk all children (including keywords) to track position relative to FROM.
    // value/identifier nodes after keyword_from are table references, not field expressions.
    let mut cursor = node.walk();
    let mut past_from = false;
    for child in node.children(&mut cursor) {
        match child.kind() {
            "keyword_from" => {
                past_from = true;
            }
            "inclusive_predicate" | "predicate" if !past_from => {
                // SELECT field expressions — resolve against the source table
                resolve_expr(&child, source, ctx, table_ref);
            }
            "where_clause" => {
                // Validate WHERE clause produces boolean
                let mut inner = child.walk();
                for wc in child.named_children(&mut inner) {
                    if wc.kind() != "keyword_where" {
                        let typ = resolve_expr(&wc, source, ctx, table_ref);
                        if !matches!(typ, Kind::Bool | Kind::Any) {
                            ctx.emit(Diagnostic::error(
                                Span::from_node(&wc),
                                Code::TypeMismatch,
                                format!("WHERE clause should be bool, got `{}`", typ),
                            ));
                        }
                    }
                }
            }
            "group_clause" => {
                // Resolve GROUP BY expressions
                let mut inner = child.walk();
                for gc in child.named_children(&mut inner) {
                    resolve_expr(&gc, source, ctx, table_ref);
                }
            }
            // value/base_value/identifier after FROM are table references — skip resolving
            // them as field expressions to avoid false "field not found" errors.
            _ => {}
        }
    }
}

/// Extract the source table name from a table_view_clause's FROM part.
///
/// Walks the children looking for value/identifier nodes after keyword_from.
fn extract_view_from_table(node: &Node, source: &str) -> Option<String> {
    use crate::parser::node_text;

    let mut cursor = node.walk();
    let mut found_from = false;
    for child in node.children(&mut cursor) {
        if child.kind() == "keyword_from" {
            found_from = true;
        } else if found_from {
            // The FROM source can be a value wrapping an identifier
            if child.kind() == "identifier" {
                return Some(node_text(&child, source).to_string());
            }
            // Or it can be a value node containing an identifier
            if child.kind() == "value" || child.kind() == "base_value" {
                if let Some(ident) = find_identifier_shallow(&child, source) {
                    return Some(ident);
                }
            }
            // Found a non-keyword after FROM, stop looking
            break;
        }
    }
    None
}

/// Find the first identifier in a node (shallow, not descending into subqueries).
fn find_identifier_shallow(node: &Node, source: &str) -> Option<String> {
    use crate::parser::node_text;

    if node.kind() == "identifier" {
        return Some(node_text(node, source).to_string());
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => return Some(node_text(&child, source).to_string()),
            "value" | "base_value" | "path" | "inclusive_predicate" => {
                if let Some(result) = find_identifier_shallow(&child, source) {
                    return Some(result);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::analyze;
    use crate::diagnostic::Code;

    #[test]
    fn assert_non_bool_warns() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string ASSERT string::len($value);
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::AssertNotBool)
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected AssertNotBool warning for non-boolean ASSERT expression, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn define_field_default_type_mismatch() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string DEFAULT 42;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::DefaultTypeMismatch)
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected DefaultTypeMismatch warning for int default on string field, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn define_field_value_type_mismatch() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string VALUE 42;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::ValueTypeMismatch)
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected ValueTypeMismatch warning for int value on string field, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn define_field_default_compatible() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string DEFAULT 'hello';
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::DefaultTypeMismatch)
            .collect();
        assert!(
            warnings.is_empty(),
            "Should not warn when DEFAULT expression matches field type: {:?}",
            warnings
        );
    }

    #[test]
    fn define_field_value_compatible() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string VALUE 'hello';
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::ValueTypeMismatch)
            .collect();
        assert!(
            warnings.is_empty(),
            "Should not warn when VALUE expression matches field type: {:?}",
            warnings
        );
    }

    #[test]
    fn define_field_default_numeric_coercion() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD score ON user TYPE number DEFAULT 42;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::DefaultTypeMismatch)
            .collect();
        assert!(
            warnings.is_empty(),
            "Should not warn when int default is assigned to number field: {:?}",
            warnings
        );
    }

    #[test]
    fn assert_bool_no_warning() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD age ON user TYPE int ASSERT $value >= 0;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::AssertNotBool)
            .collect();
        assert!(
            warnings.is_empty(),
            "Should not warn when ASSERT expression is boolean: {:?}",
            warnings
        );
    }

    #[test]
    fn define_function_return_type_mismatch() {
        let result = analyze(
            r#"
            DEFINE FUNCTION fn::get_name() -> string { RETURN 42; };
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::ReturnTypeMismatch)
            .collect();
        assert!(
            !warnings.is_empty(),
            "Expected ReturnTypeMismatch warning when function body returns int but declares string, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn define_function_return_type_compatible() {
        let result = analyze(
            r#"
            DEFINE FUNCTION fn::get_name() -> string { RETURN "hello"; };
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::ReturnTypeMismatch)
            .collect();
        assert!(
            errors.is_empty(),
            "Should not warn when function body returns compatible type: {:?}",
            errors
        );
    }

    // ── DEFINE TABLE PERMISSIONS tests ─────────────────────────

    #[test]
    fn define_table_permissions_where_bool() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL
                PERMISSIONS
                    FOR select WHERE $auth.role = 'admin'
                    FOR create, update NONE
                    FOR delete WHERE $auth.id = id;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(
            errors.is_empty(),
            "Valid permissions should not error: {:?}",
            errors
        );
    }

    #[test]
    fn define_table_permissions_non_bool_warns() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL
                PERMISSIONS
                    FOR select WHERE "not a bool";
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("bool"))
            .collect();
        assert!(!warnings.is_empty());
    }

    #[test]
    fn define_table_permissions_full_none() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL
                PERMISSIONS
                    FOR select FULL
                    FOR create, update, delete NONE;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(
            errors.is_empty(),
            "FULL/NONE permissions should not error: {:?}",
            errors
        );
    }

    #[test]
    fn define_table_permissions_global_none() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL
                PERMISSIONS NONE;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(
            errors.is_empty(),
            "Global PERMISSIONS NONE should not error: {:?}",
            errors
        );
    }

    #[test]
    fn define_table_permissions_global_full() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL
                PERMISSIONS FULL;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(
            errors.is_empty(),
            "Global PERMISSIONS FULL should not error: {:?}",
            errors
        );
    }

    #[test]
    fn define_field_permissions_where_bool() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string
                PERMISSIONS
                    FOR select WHERE $auth != NONE
                    FOR create, update FULL
                    FOR delete NONE;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(
            errors.is_empty(),
            "Valid field permissions should not error: {:?}",
            errors
        );
    }

    #[test]
    fn define_field_permissions_non_bool_warns() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string
                PERMISSIONS FOR select WHERE "not a bool";
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("bool"))
            .collect();
        assert!(!warnings.is_empty());
    }

    // ── DEFINE TABLE ... AS SELECT (view) tests ──────────────────

    #[test]
    fn define_table_view_as_validates_query() {
        let result = crate::analyze_strict(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE TABLE user_names AS SELECT name FROM user;
        "#).unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Valid view should not error: {:?}", errors);
    }

    #[test]
    fn define_table_view_as_nonexistent_table_strict() {
        let result = crate::analyze_strict(r#"
            DEFINE TABLE user_names AS SELECT name FROM nonexistent;
        "#).unwrap();
        let errors: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.is_error() && d.message.contains("nonexistent"))
            .collect();
        assert!(!errors.is_empty());
    }

    // ── CHANGEFEED tests ────────────────────────────────────────

    #[test]
    fn define_table_changefeed_no_error() {
        let result = crate::analyze(r#"
            DEFINE TABLE user CHANGEFEED 1h;
        "#).unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty());
    }

    // ── DEFINE SCOPE tests ────────────────────────────────────────

    #[test]
    fn define_scope_basic_no_error() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD email ON user TYPE string;
            DEFINE FIELD pass ON user TYPE string;
            DEFINE SCOPE user_scope
                SIGNIN (SELECT * FROM user WHERE email = $email)
                SIGNUP (CREATE user SET email = $email, pass = $pass);
        "#).unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(
            errors.is_empty(),
            "Valid DEFINE SCOPE with SIGNIN/SIGNUP should not error: {:?}",
            errors
        );
    }

    #[test]
    fn define_scope_signin_only_no_error() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD email ON user TYPE string;
            DEFINE SCOPE user_scope
                SIGNIN (SELECT * FROM user WHERE email = $email);
        "#).unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(
            errors.is_empty(),
            "DEFINE SCOPE with SIGNIN only should not error: {:?}",
            errors
        );
    }

    #[test]
    fn define_scope_signup_only_no_error() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD email ON user TYPE string;
            DEFINE FIELD pass ON user TYPE string;
            DEFINE SCOPE user_scope
                SIGNUP (CREATE user SET email = $email, pass = $pass);
        "#).unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(
            errors.is_empty(),
            "DEFINE SCOPE with SIGNUP only should not error: {:?}",
            errors
        );
    }

    #[test]
    fn define_scope_no_signin_signup_no_error() {
        let result = crate::analyze(r#"
            DEFINE SCOPE user_scope;
        "#).unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(
            errors.is_empty(),
            "DEFINE SCOPE without SIGNIN/SIGNUP should not error: {:?}",
            errors
        );
    }
}
