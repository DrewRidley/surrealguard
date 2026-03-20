//! LET statement analysis.
//!
//! Handles: LET $var = expr
//! Extracts variable name, resolves expression type, binds in scope.
//!
//! Result type: Kind::Null (LET is a statement, not an expression)

use tree_sitter::Node;

use crate::context::Context;
use crate::parser::{find_all, node_text};
use crate::scope::{Binding, BindingKind};
use crate::span::Span;
use crate::types::Kind;
use crate::v2::expr;

/// Analyze a LET statement. Binds the variable in scope.
pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();

    let mut var_name: Option<String> = None;
    let mut var_span = Span::from_node(node);
    let mut val_type = Kind::Any;

    for child in &children {
        match child.kind() {
            "keyword_let" | "=" => continue,
            "variable_name" | "variable" | "parameter" => {
                let name = node_text(child, source);
                var_name = Some(if name.starts_with('$') {
                    name.to_string()
                } else {
                    format!("${}", name)
                });
                var_span = Span::from_node(child);
            }
            _ => {
                // The remaining child is the value expression.
                // For subquery statements (e.g. SELECT), resolve recursively.
                val_type = resolve_let_value(child, source, ctx);
            }
        }
    }

    if let Some(name) = var_name {
        ctx.scope.bind(Binding {
            name,
            typ: val_type,
            span: var_span,
            mutable: true,
            kind: BindingKind::Let,
        });
    }

    Kind::Null
}

/// Resolve the type of the value in a LET expression.
/// Handles subqueries (SELECT, CREATE, etc.) and simple expressions.
fn resolve_let_value(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    match node.kind() {
        // Subquery wrapper — descend
        "subquery_statement" | "primary_statement" => {
            let mut cursor = node.walk();
            let children: Vec<_> = node.named_children(&mut cursor).collect();
            if let Some(child) = children.first() {
                resolve_let_value(child, source, ctx)
            } else {
                Kind::Any
            }
        }
        // SELECT subquery — use the select analyzer
        "select_statement" => {
            crate::v2::statements::select::analyze(node, source, ctx)
        }
        // CREATE subquery
        "create_statement" => {
            crate::v2::statements::create::analyze(node, source, ctx)
        }
        // UPDATE subquery
        "update_statement" => {
            crate::v2::statements::update::analyze(node, source, ctx)
        }
        // DELETE subquery
        "delete_statement" => {
            crate::v2::statements::delete::analyze(node, source, ctx)
        }
        // INSERT subquery
        "insert_statement" => {
            crate::v2::statements::insert::analyze(node, source, ctx)
        }
        // UPSERT subquery
        "upsert_statement" => {
            crate::v2::statements::upsert::analyze(node, source, ctx)
        }
        // Simple expression (literal, variable, function call, etc.)
        _ => expr::resolve_simple(node, source, ctx, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{analyze_with_context, Context};

    fn test_schema() -> Context {
        let mut ctx = Context::new();
        let _ = analyze_with_context(
            r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
                DEFINE FIELD age ON user TYPE int;
                DEFINE FIELD email ON user TYPE string;
                DEFINE FIELD active ON user TYPE bool;
                DEFINE FIELD tags ON user TYPE array<string>;
                DEFINE FIELD profile ON user TYPE record<profile>;

            DEFINE TABLE profile SCHEMAFULL;
                DEFINE FIELD bio ON profile TYPE string;
                DEFINE FIELD avatar ON profile TYPE string;

            DEFINE TABLE post SCHEMAFULL;
                DEFINE FIELD title ON post TYPE string;
                DEFINE FIELD body ON post TYPE string;
                DEFINE FIELD author ON post TYPE record<user>;
                DEFINE FIELD created_at ON post TYPE datetime;

            DEFINE TABLE wrote TYPE RELATION FROM user TO post;
                DEFINE FIELD created_at ON wrote TYPE datetime;

            DEFINE TABLE follows TYPE RELATION FROM user TO user;
            "#,
            &mut ctx,
        );
        ctx.take_diagnostics();
        ctx
    }

    fn query_type(query: &str) -> Kind {
        let mut ctx = test_schema();
        let full = format!("LET $result = {};", query);
        let _ = analyze_with_context(&full, &mut ctx);
        ctx.take_diagnostics();
        ctx.scope
            .lookup("$result")
            .map(|b| b.typ.clone())
            .unwrap_or(Kind::Any)
    }

    fn let_binding_type(stmt: &str) -> Kind {
        let mut ctx = test_schema();
        let _ = analyze_with_context(stmt, &mut ctx);
        ctx.take_diagnostics();
        // Extract the variable name from the statement
        let var = stmt.split_whitespace()
            .find(|w| w.starts_with('$'))
            .map(|w| w.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '$'))
            .unwrap_or("$result");
        ctx.scope
            .lookup(var)
            .map(|b| b.typ.clone())
            .unwrap_or(Kind::Any)
    }

    // ── LET with literal ─────────────────────────────────────

    #[test]
    fn let_int_literal() {
        let typ = let_binding_type("LET $x = 42;");
        assert!(
            matches!(typ, Kind::Int | Kind::Number),
            "Expected int or number, got: {:?}", typ
        );
    }

    #[test]
    fn let_string_literal() {
        let typ = let_binding_type("LET $greeting = 'hello';");
        assert_eq!(typ, Kind::String, "Expected string, got: {:?}", typ);
    }

    #[test]
    fn let_bool_literal() {
        let typ = let_binding_type("LET $flag = true;");
        assert_eq!(typ, Kind::Bool, "Expected bool, got: {:?}", typ);
    }

    // ── LET with subquery ────────────────────────────────────

    #[test]
    fn let_with_select_subquery() {
        let typ = query_type("SELECT * FROM user");
        assert!(matches!(&typ, Kind::Array(_, _)), "Expected array, got: {:?}", typ);
    }

    #[test]
    fn let_with_select_value() {
        let typ = query_type("SELECT VALUE name FROM user");
        assert_eq!(typ, Kind::Array(Box::new(Kind::String), None));
    }
}
