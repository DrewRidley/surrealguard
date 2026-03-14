use tree_sitter::Node;

use crate::context::Context;
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::node_text;
use crate::resolve::resolve_expr;
use crate::span::Span;
use crate::types::Kind;

/// Analyze a SHOW CHANGES statement.
///
/// Grammar:
///   show_statement = keyword_show + keyword_changes + keyword_for
///                  + choice(keyword_table + identifier, keyword_database)
///                  + keyword_since + value + optional(limit_clause)
///
/// Validations:
/// - In strict mode, if targeting a table, validate the table is defined.
/// - The SINCE argument must be a datetime type.
pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    let mut cursor = node.walk();
    let mut past_table_keyword = false;
    let mut past_since_keyword = false;
    let mut table_name: Option<String> = None;

    for child in node.children(&mut cursor) {
        let kind = child.kind();
        match kind {
            "keyword_table" => {
                past_table_keyword = true;
            }
            "identifier" if past_table_keyword && table_name.is_none() => {
                table_name = Some(node_text(&child, source).to_string());

                // In strict mode, validate the table exists
                if ctx.strict && !ctx.has_table(node_text(&child, source)) {
                    ctx.emit(Diagnostic::error(
                        Span::from_node(&child),
                        Code::TableNotFound,
                        format!("table `{}` not defined", node_text(&child, source)),
                    ));
                }
            }
            "keyword_since" => {
                past_since_keyword = true;
            }
            "value" | "base_value" if past_since_keyword => {
                let typ = resolve_expr(&child, source, ctx, None);
                if typ != Kind::Datetime && typ != Kind::Any {
                    ctx.emit(Diagnostic::warning(
                        Span::from_node(&child),
                        Code::TypeMismatch,
                        format!("SHOW CHANGES SINCE expects a datetime, got `{}`", typ),
                    ));
                }
                // Only validate the first value after SINCE
                past_since_keyword = false;
            }
            _ => {}
        }
    }

    Kind::Null
}

#[cfg(test)]
mod tests {
    #[test]
    fn show_changes_with_datetime_no_warning() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            SHOW CHANGES FOR TABLE user SINCE d"2023-01-01T00:00:00Z";
        "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("SHOW CHANGES"))
            .collect();
        assert!(
            warnings.is_empty(),
            "SHOW CHANGES with datetime should not warn: {:?}",
            warnings
        );
    }

    #[test]
    fn show_changes_with_string_warns() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            SHOW CHANGES FOR TABLE user SINCE "not-a-datetime";
        "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("SHOW CHANGES"))
            .collect();
        assert!(
            !warnings.is_empty(),
            "SHOW CHANGES with string should warn about type mismatch"
        );
    }

    #[test]
    fn show_changes_with_number_warns() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            SHOW CHANGES FOR TABLE user SINCE 42;
        "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("SHOW CHANGES"))
            .collect();
        assert!(
            !warnings.is_empty(),
            "SHOW CHANGES with number should warn about type mismatch"
        );
    }

    #[test]
    fn show_changes_strict_undefined_table() {
        let result = crate::analyze_strict(
            r#"SHOW CHANGES FOR TABLE nonexistent SINCE d"2023-01-01T00:00:00Z";"#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("nonexistent"))
            .collect();
        assert!(
            !errors.is_empty(),
            "SHOW CHANGES in strict mode should error on undefined table"
        );
    }

    #[test]
    fn show_changes_strict_defined_table_no_error() {
        let result = crate::analyze_strict(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            SHOW CHANGES FOR TABLE user SINCE d"2023-01-01T00:00:00Z";
        "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("user"))
            .collect();
        assert!(
            errors.is_empty(),
            "SHOW CHANGES with defined table should not error: {:?}",
            errors
        );
    }

    #[test]
    fn show_changes_with_param_no_warning() {
        let result = crate::analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            SHOW CHANGES FOR TABLE user SINCE $last_sync;
        "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("SHOW CHANGES"))
            .collect();
        assert!(
            warnings.is_empty(),
            "SHOW CHANGES with param (Any type) should not warn: {:?}",
            warnings
        );
    }
}
