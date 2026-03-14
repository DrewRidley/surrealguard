/// ALTER TABLE statement analysis.
///
/// Tracks schema modifications so that subsequent queries see
/// the updated table properties (schema mode, drop flag).
use tree_sitter::Node;

use crate::context::{Context, SchemaMode};
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::node_text;
use crate::span::Span;
use crate::types::Kind;

pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    // Extract table name — it's the identifier child after ALTER TABLE
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

    let table_name = match table_name {
        Some(n) => n,
        None => return Kind::Null,
    };

    // Check that the table exists
    if !ctx.has_table(&table_name) {
        // In strict mode, error on unknown table; otherwise just return
        if ctx.strict {
            ctx.emit(Diagnostic::error(
                Span::from_node(node),
                Code::TableNotFound,
                format!("cannot ALTER unknown table `{}`", table_name),
            ));
        }
        return Kind::Null;
    }

    // Walk children to find schema mode and drop flag changes
    let mut new_schema_mode = None;
    let mut set_drop = false;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "keyword_schemafull" => {
                new_schema_mode = Some(SchemaMode::Schemafull);
            }
            "keyword_schemaless" => {
                new_schema_mode = Some(SchemaMode::Schemaless);
            }
            "keyword_drop" => {
                set_drop = true;
            }
            _ => {}
        }
    }

    // Apply modifications to the table definition
    if let Some(table_def) = ctx.get_table_mut(&table_name) {
        if let Some(mode) = new_schema_mode {
            table_def.schema_mode = mode;
        }
        if set_drop {
            table_def.drop = true;
        }
    }

    Kind::Null
}

#[cfg(test)]
mod tests {
    use crate::analyze;
    use crate::diagnostic::Code;

    #[test]
    fn alter_table_schemafull_affects_subsequent_queries() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMALESS;
            DEFINE FIELD name ON user TYPE string;
            ALTER TABLE user SCHEMAFULL;
            UPDATE user SET nonexistent = 1;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::UndefinedFieldOnSchemafull)
            .collect();
        assert!(
            !warnings.is_empty(),
            "After ALTER TABLE to SCHEMAFULL, setting an undefined field should warn: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn alter_table_schemaless_allows_arbitrary_fields() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            ALTER TABLE user SCHEMALESS;
            UPDATE user SET anything = 1;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::UndefinedFieldOnSchemafull)
            .collect();
        assert!(
            warnings.is_empty(),
            "After ALTER TABLE to SCHEMALESS, arbitrary fields should be allowed: {:?}",
            warnings
        );
    }

    #[test]
    fn alter_table_drop_is_tracked() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMALESS;
            ALTER TABLE user DROP;
            "#,
        )
        .unwrap();
        // DROP should be tracked without errors
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(
            errors.is_empty(),
            "ALTER TABLE DROP should not produce errors: {:?}",
            errors
        );
        // Verify the drop flag was set
        assert!(
            result.context.get_table("user").unwrap().drop,
            "ALTER TABLE DROP should set the drop flag"
        );
    }

    #[test]
    fn alter_unknown_table_strict_mode() {
        let result = crate::analyze_strict(
            r#"
            ALTER TABLE nonexistent SCHEMAFULL;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::TableNotFound)
            .collect();
        assert!(
            !errors.is_empty(),
            "ALTER on unknown table in strict mode should error: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn alter_unknown_table_non_strict_no_error() {
        let result = analyze(
            r#"
            ALTER TABLE nonexistent SCHEMAFULL;
            "#,
        )
        .unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(
            errors.is_empty(),
            "ALTER on unknown table in non-strict mode should not error: {:?}",
            errors
        );
    }

    #[test]
    fn alter_table_schemafull_with_defined_fields_no_false_positive() {
        let result = analyze(
            r#"
            DEFINE TABLE user SCHEMALESS;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            ALTER TABLE user SCHEMAFULL;
            UPDATE user SET name = 'Alice', age = 30;
            "#,
        )
        .unwrap();
        let warnings: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::UndefinedFieldOnSchemafull)
            .collect();
        assert!(
            warnings.is_empty(),
            "Defined fields should not warn after ALTER TABLE SCHEMAFULL: {:?}",
            warnings
        );
    }
}
