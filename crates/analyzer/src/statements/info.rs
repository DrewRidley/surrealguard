use tree_sitter::Node;

use crate::context::Context;
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{child_by_kind, find_all, node_text};
use crate::span::Span;
use crate::types::Kind;

/// Analyze an INFO FOR statement.
///
/// In strict mode, checks that `INFO FOR TABLE <name>` references
/// a table that has been defined in the schema context.
pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    if !ctx.strict {
        return Kind::Null;
    }

    // info_statement -> info_target -> table_info -> identifier
    if let Some(info_target) = child_by_kind(node, "info_target") {
        if let Some(table_info) = child_by_kind(&info_target, "table_info") {
            let identifiers = find_all(&table_info, "identifier");
            if let Some(ident) = identifiers.first() {
                let table_name = node_text(ident, source);
                if !ctx.has_table(table_name) {
                    ctx.emit(Diagnostic::error(
                        Span::from_node(node),
                        Code::TableNotFound,
                        format!("table `{}` not defined", table_name),
                    ));
                }
            }
        }
    }

    Kind::Null
}

#[cfg(test)]
mod tests {
    #[test]
    fn info_for_table_exists_no_error() {
        let result = crate::analyze_strict(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            INFO FOR TABLE user;
        "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty());
    }

    #[test]
    fn info_for_table_not_exists_strict() {
        let result = crate::analyze_strict(
            r#"
            INFO FOR TABLE nonexistent;
        "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.is_error() && d.message.contains("nonexistent"))
            .collect();
        assert!(!errors.is_empty());
    }

    #[test]
    fn info_for_db_no_error_strict() {
        let result = crate::analyze_strict(
            r#"
            INFO FOR DB;
        "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty());
    }

    #[test]
    fn info_for_ns_no_error_strict() {
        let result = crate::analyze_strict(
            r#"
            INFO FOR NS;
        "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty());
    }
}
