/// Permission clause validation.
///
/// Validates that PERMISSIONS clauses reference valid fields on the table.
/// Sets up $this and $auth scope for the permission expression.
use tree_sitter::Node;

use crate::context::Context;
use crate::diagnostic::{Code, Diagnostic};
use crate::resolve::resolve_expr;
use crate::scope::{Binding, BindingKind, ScopeKind};
use crate::span::Span;
use crate::types::Kind;

/// Analyze all permission clauses in the CST.
///
/// Note: DEFINE TABLE and DEFINE FIELD permission clauses are now analyzed
/// inline by `statements::define::analyze_table` and `statements::define::analyze_field`.
/// This function remains as an entry point for any additional permission validation
/// that operates on the full tree (e.g., cross-statement permission checks).
pub fn analyze_permissions(_root: &Node, _source: &str, _ctx: &mut Context) {
    // Permission clause analysis is now handled by the define statement analyzer
    // (statements/define.rs) which calls analyze_permission_clause directly.
    // This avoids duplicate analysis since both define.rs and this module
    // previously processed the same permission clauses.
}

pub fn analyze_permission_clause(
    node: &Node,
    source: &str,
    ctx: &mut Context,
    table: Option<&str>,
) {
    // Set up $this scope for the table
    ctx.scope.push(ScopeKind::Permissions);

    // Bind implicit permission-scope variables
    let perm_span = Span::from_node(node);

    // $auth — the authenticated user record (object type)
    ctx.scope.bind(Binding {
        name: "$auth".into(),
        typ: Kind::Any,
        span: perm_span,
        mutable: false,
        kind: BindingKind::Implicit,
    });

    // $token — JWT token claims (object)
    ctx.scope.bind(Binding {
        name: "$token".into(),
        typ: Kind::Any,
        span: perm_span,
        mutable: false,
        kind: BindingKind::Implicit,
    });

    // $scope — the current scope name (string)
    ctx.scope.bind(Binding {
        name: "$scope".into(),
        typ: Kind::String,
        span: perm_span,
        mutable: false,
        kind: BindingKind::Implicit,
    });

    // Validate table exists before setting $this
    if let Some(tbl) = table {
        if !ctx.has_table(tbl) {
            // Skip permission analysis if table doesn't exist
            // (table-level error already emitted elsewhere)
            ctx.scope.pop();
            return;
        }
    }

    if let Some(tbl) = table {
        let table_type = ctx
            .build_table_type(tbl)
            .unwrap_or(Kind::Object);
        ctx.scope
            .set_this(table_type, Span::from_node(node));
    }

    // Resolve all expressions within the permission clause
    resolve_permission_children(node, source, ctx, table);

    ctx.scope.pop();
}

/// Recursively resolve expressions within permission clause nodes.
fn resolve_permission_children(node: &Node, source: &str, ctx: &mut Context, table: Option<&str>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            // Skip keywords
            "keyword_permissions" | "keyword_for" | "keyword_select"
            | "keyword_create" | "keyword_update" | "keyword_delete"
            | "keyword_where" | "keyword_full" | "keyword_none"
            | "keyword_true" | "keyword_false" => {}
            // Recurse into structural nodes
            "permission_for_clause" | "permissions_for_clause"
            | "permission_clause_body" | "where_clause" => {
                resolve_permission_children(&child, source, ctx, table);
            }
            // Resolve expressions and check type
            _ => {
                let typ = resolve_expr(&child, source, ctx, table);
                check_permission_type(&typ, &child, ctx);
            }
        }
    }
}

/// Check that a permission condition evaluates to bool.
fn check_permission_type(typ: &Kind, node: &Node, ctx: &mut Context) {
    if !matches!(typ, Kind::Bool | Kind::Any) {
        let span = Span::from_node(node);
        ctx.emit(Diagnostic::error(
            span,
            Code::PermissionConditionNotBool,
            format!("permission condition must evaluate to bool, got `{}`", typ),
        ));
    }
}

#[cfg(test)]
mod tests {
    use crate::diagnostic::Code;

    #[test]
    fn permission_non_bool_condition_produces_error() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string
                PERMISSIONS FOR select WHERE "not_a_bool";
            "#,
        )
        .unwrap();

        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::PermissionConditionNotBool)
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected PermissionConditionNotBool error, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn permission_implicit_variables_accessible() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string
                PERMISSIONS FOR select WHERE $auth != NONE;
            "#,
        )
        .unwrap();

        let undefined_errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::UndefinedVariable)
            .collect();
        assert!(
            undefined_errors.is_empty(),
            "Expected no UndefinedVariable errors for $auth, got: {:?}",
            undefined_errors
        );
    }

    #[test]
    fn permission_token_and_scope_accessible() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string
                PERMISSIONS FOR select WHERE $token != NONE AND $scope != NONE;
            "#,
        )
        .unwrap();

        let undefined_errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::UndefinedVariable)
            .collect();
        assert!(
            undefined_errors.is_empty(),
            "Expected no UndefinedVariable errors for $token/$scope, got: {:?}",
            undefined_errors
        );
    }

    #[test]
    fn permission_on_nonexistent_table_does_not_crash() {
        let result = crate::analyze(
            r#"
            DEFINE FIELD name ON nonexistent TYPE string
                PERMISSIONS FOR select WHERE $auth != NONE;
            "#,
        );
        // Should not panic — the result should be Ok
        assert!(result.is_ok(), "Analysis should not crash on nonexistent table");
    }
}
