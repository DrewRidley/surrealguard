use tree_sitter::Node;

use crate::context::Context;
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{child_by_kind, node_text};
use crate::span::Span;
use crate::types::Kind;

/// Analyze a REMOVE statement.
///
/// In strict mode, checks that the entity being removed actually exists
/// in the schema context. Supports REMOVE TABLE, FIELD, INDEX, EVENT,
/// FUNCTION, and PARAM.
pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    if !ctx.strict {
        return Kind::Null;
    }

    // Check for IF EXISTS clause — if present, the user explicitly handles
    // the case where the entity might not exist, so skip validation.
    if child_by_kind(node, "if_exists_clause").is_some() {
        return Kind::Null;
    }

    // Walk children to determine the variant and extract names.
    // The remove_statement structure is:
    //   REMOVE <keyword_type> [IF EXISTS] <identifier> [ON [TABLE] <identifier>]
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();

    // Find the variant keyword (skip keyword_remove)
    let mut variant: Option<&str> = None;
    let mut identifiers: Vec<String> = Vec::new();
    let mut custom_function_name: Option<String> = None;
    let mut variable_name: Option<String> = None;

    for child in &children {
        match child.kind() {
            "keyword_remove" => {}
            k if k.starts_with("keyword_") && variant.is_none() => {
                variant = Some(k);
            }
            "identifier" => {
                identifiers.push(node_text(child, source).to_string());
            }
            "custom_function_name" => {
                custom_function_name = Some(node_text(child, source).to_string());
            }
            "variable_name" => {
                variable_name = Some(node_text(child, source).to_string());
            }
            _ => {}
        }
    }

    let span = Span::from_node(node);

    match variant {
        Some("keyword_table") => {
            // REMOVE TABLE <name>
            if let Some(name) = identifiers.first() {
                if !ctx.has_table(name) {
                    ctx.emit(Diagnostic::error(
                        span,
                        Code::TableNotFound,
                        format!("table `{}` not defined", name),
                    ));
                }
            }
        }
        Some("keyword_field") => {
            // REMOVE FIELD <field_name> ON [TABLE] <table_name>
            // First identifier is field name, last identifier is table name
            if identifiers.len() >= 2 {
                let field_name = &identifiers[0];
                let table_name = &identifiers[identifiers.len() - 1];
                if !ctx.has_table(table_name) {
                    ctx.emit(Diagnostic::error(
                        span,
                        Code::TableNotFound,
                        format!("table `{}` not defined", table_name),
                    ));
                } else if ctx.get_field(table_name, field_name).is_none() {
                    ctx.emit(Diagnostic::error(
                        span,
                        Code::FieldNotFound,
                        format!(
                            "field `{}` not defined on table `{}`",
                            field_name, table_name
                        ),
                    ));
                }
            }
        }
        Some("keyword_index") => {
            // REMOVE INDEX <index_name> ON [TABLE] <table_name>
            if identifiers.len() >= 2 {
                let index_name = &identifiers[0];
                let table_name = &identifiers[identifiers.len() - 1];
                if !ctx.has_table(table_name) {
                    ctx.emit(Diagnostic::error(
                        span,
                        Code::TableNotFound,
                        format!("table `{}` not defined", table_name),
                    ));
                } else if !ctx.has_index(table_name, index_name) {
                    ctx.emit(Diagnostic::error(
                        span,
                        Code::FieldNotFound,
                        format!(
                            "index `{}` not defined on table `{}`",
                            index_name, table_name
                        ),
                    ));
                }
            }
        }
        Some("keyword_event") => {
            // REMOVE EVENT <event_name> ON [TABLE] <table_name>
            // Events are not tracked in the context, so we can only
            // validate the table exists.
            if identifiers.len() >= 2 {
                let table_name = &identifiers[identifiers.len() - 1];
                if !ctx.has_table(table_name) {
                    ctx.emit(Diagnostic::error(
                        span,
                        Code::TableNotFound,
                        format!("table `{}` not defined", table_name),
                    ));
                }
            }
        }
        Some("keyword_function") => {
            // REMOVE FUNCTION fn::name
            if let Some(fn_name) = custom_function_name {
                // Normalize: the grammar captures "fn::name", ctx stores "name"
                let lookup = fn_name.strip_prefix("fn::").unwrap_or(&fn_name);
                if ctx.get_function(lookup).is_none() {
                    ctx.emit(Diagnostic::error(
                        span,
                        Code::FunctionNotFound,
                        format!("function `{}` not defined", fn_name),
                    ));
                }
            }
        }
        Some("keyword_param") => {
            // REMOVE PARAM $name
            if let Some(var) = variable_name {
                let param = var.strip_prefix('$').unwrap_or(&var);
                if ctx.scope.lookup(param).is_none() {
                    ctx.emit(Diagnostic::error(
                        span,
                        Code::ParameterNotFound,
                        format!("parameter `{}` not defined", var),
                    ));
                }
            }
        }
        // For NAMESPACE, DATABASE, USER, TOKEN, SCOPE, ANALYZER, ACCESS,
        // MODULE, API, BUCKET — we don't track these in the context, so
        // no validation is possible.
        _ => {}
    }

    Kind::Null
}

#[cfg(test)]
mod tests {
    #[test]
    fn remove_table_exists_no_error() {
        let result = crate::analyze_strict(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            REMOVE TABLE user;
        "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("user"))
            .collect();
        assert!(errors.is_empty(), "should not error when table exists");
    }

    #[test]
    fn remove_table_not_exists_strict() {
        let result = crate::analyze_strict(
            r#"
            REMOVE TABLE nonexistent;
        "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("nonexistent"))
            .collect();
        assert!(!errors.is_empty(), "should error when table does not exist");
    }

    #[test]
    fn remove_table_not_exists_lenient() {
        let result = crate::analyze(
            r#"
            REMOVE TABLE nonexistent;
        "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .collect();
        assert!(errors.is_empty(), "lenient mode should not error");
    }

    #[test]
    fn remove_table_if_exists_no_error() {
        let result = crate::analyze_strict(
            r#"
            REMOVE TABLE IF EXISTS nonexistent;
        "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("nonexistent"))
            .collect();
        assert!(
            errors.is_empty(),
            "IF EXISTS should suppress strict error"
        );
    }

    #[test]
    fn remove_field_exists_no_error() {
        let result = crate::analyze_strict(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            REMOVE FIELD name ON user;
        "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("name"))
            .collect();
        assert!(errors.is_empty(), "should not error when field exists");
    }

    #[test]
    fn remove_field_not_exists_strict() {
        let result = crate::analyze_strict(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            REMOVE FIELD email ON user;
        "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("email"))
            .collect();
        assert!(!errors.is_empty(), "should error when field not defined");
    }

    #[test]
    fn remove_field_table_not_exists_strict() {
        let result = crate::analyze_strict(
            r#"
            REMOVE FIELD name ON nonexistent;
        "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("nonexistent"))
            .collect();
        assert!(
            !errors.is_empty(),
            "should error when table does not exist"
        );
    }

    #[test]
    fn remove_index_table_not_exists_strict() {
        let result = crate::analyze_strict(
            r#"
            REMOVE INDEX idx_name ON nonexistent;
        "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("nonexistent"))
            .collect();
        assert!(
            !errors.is_empty(),
            "should error when table does not exist"
        );
    }
}
