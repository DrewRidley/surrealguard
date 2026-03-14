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
                .or_else(|| child_by_kind(node, "delete_target").map(|t| Span::from_node(&t)))
                .unwrap_or_else(|| Span::from_node(node));
            ctx.emit(Diagnostic::error(
                span,
                Code::TableNotFound,
                format!("table `{}` not defined", tbl),
            ));
        }
    }

    // Analyze WHERE clause and RETURN clause
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "return_clause" {
            let mut inner = child.walk();
            for c in child.named_children(&mut inner) {
                match c.kind() {
                    k if k.starts_with("keyword_") => {}
                    _ => {
                        resolve_expr(&c, source, ctx, table_ref);
                        // Validate that referenced fields exist on schemafull tables
                        if let Some(tbl) = table_ref {
                            let is_schemafull = ctx
                                .get_table(tbl)
                                .map(|t| t.schema_mode == SchemaMode::Schemafull)
                                .unwrap_or(false);
                            if is_schemafull {
                                for ident in &find_all(&c, "identifier") {
                                    let field_name = node_text(ident, source);
                                    if field_name != "id" && ctx.get_field(tbl, field_name).is_none() {
                                        let span = Span::from_node(ident);
                                        ctx.emit(Diagnostic::warning(
                                            span,
                                            Code::FieldNotFound,
                                            format!(
                                                "field `{}` in RETURN clause is not defined on table `{}`",
                                                field_name, tbl
                                            ),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        } else if child.kind() == "where_clause" {
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
    }

    super::compute_dml_result_type(node, source, ctx, table_ref)
}

#[cfg(test)]
mod tests {
    use crate::analyze;

    #[test]
    fn delete_return_valid_field_no_warning() {
        let result = analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            DELETE user RETURN name;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::Code::FieldNotFound && d.message.contains("RETURN"))
            .collect();
        assert!(warnings.is_empty(), "No warning expected for valid RETURN field: {:?}", warnings);
    }

    #[test]
    fn delete_return_undefined_field_warns() {
        let result = analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DELETE user RETURN nonexistent;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::Code::FieldNotFound && d.message.contains("RETURN"))
            .collect();
        assert!(!warnings.is_empty(), "Expected warning for undefined field in RETURN clause, got: {:?}", result.diagnostics);
        assert!(
            warnings[0].message.contains("nonexistent"),
            "Warning should mention the field name, got: {}",
            warnings[0].message
        );
    }

    #[test]
    fn delete_return_id_no_warning() {
        let result = analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DELETE user RETURN id;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::Code::FieldNotFound && d.message.contains("RETURN"))
            .collect();
        assert!(warnings.is_empty(), "No warning expected for RETURN id: {:?}", warnings);
    }
}
