/// Schema extraction from tree-sitter CST.
///
/// Walks DEFINE TABLE and DEFINE FIELD statements in the CST
/// and populates the [`Context`] with structured definitions.
use tree_sitter::Node;

use super::context::{Context, FieldDef, SchemaMode, TableDef, TableKind};
use super::parser::{child_by_kind, children_by_kind, node_text};
use super::types::Type;

/// Extract all schema definitions from a parsed CST and add them to the context.
pub fn extract_schema(root: &Node, source: &str, ctx: &mut Context) {
    extract_tables(root, source, ctx);
    extract_fields(root, source, ctx);
}

fn extract_tables(root: &Node, source: &str, ctx: &mut Context) {
    for node in super::parser::find_all(root, "define_table_statement") {
        if let Some(def) = parse_table_def(&node, source) {
            ctx.define_table(def);
        }
    }
}

fn extract_fields(root: &Node, source: &str, ctx: &mut Context) {
    for node in super::parser::find_all(root, "define_field_statement") {
        if let Some(def) = parse_field_def(&node, source) {
            ctx.define_field(def);
        }
    }
}

fn parse_table_def(node: &Node, source: &str) -> Option<TableDef> {
    // Find the table name — the identifier after DEFINE TABLE [OVERWRITE] [IF NOT EXISTS]
    let name = find_table_name(node, source)?;

    let has_keyword = |kind: &str| child_by_kind(node, kind).is_some();

    let schema_mode = if has_keyword("keyword_schemafull") {
        SchemaMode::Schemafull
    } else {
        SchemaMode::Schemaless
    };

    let drop = has_keyword("keyword_drop");

    let kind = if let Some(type_clause) = child_by_kind(node, "table_type_clause") {
        parse_table_kind(&type_clause, source)
    } else {
        TableKind::Normal
    };

    Some(TableDef {
        name,
        schema_mode,
        kind,
        drop,
    })
}

fn find_table_name(node: &Node, source: &str) -> Option<String> {
    // The identifier directly under define_table_statement (not nested in clauses)
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            return Some(node_text(&child, source).to_string());
        }
    }
    None
}

fn parse_table_kind(node: &Node, source: &str) -> TableKind {
    if child_by_kind(node, "keyword_any").is_some() {
        return TableKind::Any;
    }
    if child_by_kind(node, "keyword_normal").is_some() {
        return TableKind::Normal;
    }
    if child_by_kind(node, "keyword_relation").is_some() {
        // Extract IN/FROM and OUT/TO tables
        let from = extract_relation_tables(node, source, true);
        let to = extract_relation_tables(node, source, false);
        return TableKind::Relation { from, to };
    }
    TableKind::Normal
}

fn extract_relation_tables(node: &Node, source: &str, is_from: bool) -> Option<Vec<String>> {
    // Look for record_or_separated nodes after keyword_in/keyword_from or keyword_out/keyword_to
    let mut cursor = node.walk();
    let mut found_direction = false;
    for child in node.children(&mut cursor) {
        if !found_direction {
            let kind = child.kind();
            if is_from && (kind == "keyword_in" || kind == "keyword_from") {
                found_direction = true;
            } else if !is_from && (kind == "keyword_out" || kind == "keyword_to") {
                found_direction = true;
            }
        } else if child.kind() == "record_or_separated" {
            let tables = extract_identifiers_from_separated(&child, source);
            return Some(tables);
        }
    }
    None
}

fn extract_identifiers_from_separated(node: &Node, source: &str) -> Vec<String> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|c| c.kind() == "identifier")
        .map(|c| node_text(&c, source).to_string())
        .collect()
}

fn parse_field_def(node: &Node, source: &str) -> Option<FieldDef> {
    let table = find_field_table(node, source)?;
    let name = find_field_name(node, source)?;

    let typ = child_by_kind(node, "type_clause")
        .and_then(|tc| parse_type_from_clause(&tc, source));

    let readonly = child_by_kind(node, "readonly_clause").is_some();
    let is_computed = child_by_kind(node, "computed_clause").is_some();

    Some(FieldDef {
        name,
        table,
        typ,
        default: None,
        readonly,
        is_computed,
    })
}

fn find_field_table(node: &Node, source: &str) -> Option<String> {
    let on_clause = child_by_kind(node, "on_table_clause")?;
    let ident = child_by_kind(&on_clause, "identifier")?;
    Some(node_text(&ident, source).to_string())
}

fn find_field_name(node: &Node, source: &str) -> Option<String> {
    // The field name is in the inclusive_predicate > predicate > value > base_value > identifier
    let pred = child_by_kind(node, "inclusive_predicate")?;
    let identifiers = super::parser::find_all(&pred, "identifier");
    if let Some(first) = identifiers.first() {
        Some(node_text(first, source).to_string())
    } else {
        None
    }
}

/// Parse a SurrealQL type from a `type_clause` node.
fn parse_type_from_clause(node: &Node, source: &str) -> Option<Type> {
    let type_node = child_by_kind(node, "type")?;
    parse_type(&type_node, source)
}

/// Parse a `type` node into our Type representation.
pub fn parse_type(node: &Node, source: &str) -> Option<Type> {
    match node.kind() {
        "type" => {
            // A type node can contain: type_name, parameterized_type, union_type, composite_type, type_object
            let mut cursor = node.walk();
            let child = node.named_children(&mut cursor).next()?;
            parse_type(&child, source)
        }
        "type_name" => {
            let name = node_text(node, source);
            Some(type_name_to_type(name))
        }
        "parameterized_type" => {
            let name_node = child_by_kind(node, "type_name")?;
            let name = node_text(&name_node, source);
            let type_args: Vec<Type> = children_by_kind(node, "type")
                .iter()
                .filter_map(|t| parse_type(t, source))
                .collect();

            match name {
                "array" => {
                    let inner = type_args.into_iter().next().unwrap_or(Type::Any);
                    Some(Type::Array(Box::new(inner), None))
                }
                "set" => {
                    let inner = type_args.into_iter().next().unwrap_or(Type::Any);
                    Some(Type::Set(Box::new(inner), None))
                }
                "option" => {
                    let inner = type_args.into_iter().next().unwrap_or(Type::Any);
                    Some(Type::Option(Box::new(inner)))
                }
                "record" => {
                    // For record<table>, the type args are type_names which we
                    // need to extract as table names, not types.
                    let table_names: Vec<String> = children_by_kind(node, "type")
                        .iter()
                        .filter_map(|t| {
                            let tn = child_by_kind(t, "type_name")?;
                            Some(node_text(&tn, source).to_string())
                        })
                        .collect();

                    if table_names.is_empty() {
                        Some(Type::Record(vec![]))
                    } else {
                        Some(Type::Record(table_names))
                    }
                }
                "geometry" => {
                    let variants: Vec<String> = children_by_kind(node, "type")
                        .iter()
                        .filter_map(|t| {
                            let tn = child_by_kind(t, "type_name")?;
                            Some(node_text(&tn, source).to_string())
                        })
                        .collect();
                    Some(Type::Geometry(variants))
                }
                _ => Some(Type::Any),
            }
        }
        "union_type" => {
            let mut cursor = node.walk();
            let types: Vec<Type> = node
                .named_children(&mut cursor)
                .filter_map(|child| parse_type(&child, source))
                .collect();
            if types.len() == 1 {
                Some(types.into_iter().next().unwrap())
            } else {
                Some(Type::Either(types))
            }
        }
        _ => None,
    }
}

/// Map a SurrealQL type name string to our Type enum.
fn type_name_to_type(name: &str) -> Type {
    match name {
        "any" => Type::Any,
        "null" | "none" => Type::Null,
        "bool" => Type::Bool,
        "int" => Type::Int,
        "float" => Type::Float,
        "decimal" => Type::Decimal,
        "number" => Type::Number,
        "string" => Type::String,
        "bytes" => Type::Bytes,
        "duration" => Type::Duration,
        "datetime" => Type::Datetime,
        "uuid" => Type::Uuid,
        "object" => Type::Object(Default::default()),
        "range" => Type::Range,
        // Unknown type names are treated as Any.
        // In practice these are often table names used in record<T>.
        _ => Type::Any,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::parser;

    fn ctx_from(source: &str) -> Context {
        let tree = parser::parse(source).unwrap();
        let root = tree.root_node();
        let mut ctx = Context::new();
        extract_schema(&root, source, &mut ctx);
        ctx
    }

    #[test]
    fn extract_schemafull_table() {
        let ctx = ctx_from("DEFINE TABLE user SCHEMAFULL;");
        let table = ctx.get_table("user").unwrap();
        assert_eq!(table.name, "user");
        assert_eq!(table.schema_mode, SchemaMode::Schemafull);
        assert!(!table.drop);
    }

    #[test]
    fn extract_drop_table() {
        let ctx = ctx_from("DEFINE TABLE temp DROP SCHEMALESS;");
        let table = ctx.get_table("temp").unwrap();
        assert!(table.drop);
        assert_eq!(table.schema_mode, SchemaMode::Schemaless);
    }

    #[test]
    fn extract_relation_table() {
        let ctx = ctx_from("DEFINE TABLE knows TYPE RELATION IN person OUT person;");
        let table = ctx.get_table("knows").unwrap();
        if let TableKind::Relation { from, to } = &table.kind {
            assert_eq!(from.as_ref().unwrap(), &["person"]);
            assert_eq!(to.as_ref().unwrap(), &["person"]);
        } else {
            panic!("Expected relation table kind");
        }
    }

    #[test]
    fn extract_field_string_type() {
        let ctx = ctx_from(
            "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string;",
        );
        let field = ctx.get_field("user", "name").unwrap();
        assert_eq!(field.typ, Some(Type::String));
    }

    #[test]
    fn extract_field_record_type() {
        let ctx = ctx_from(
            "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD org ON user TYPE record<organization>;",
        );
        let field = ctx.get_field("user", "org").unwrap();
        assert_eq!(field.typ, Some(Type::Record(vec!["organization".to_string()])));
    }

    #[test]
    fn extract_field_option_array_type() {
        let ctx = ctx_from(
            "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD tags ON user TYPE option<array<string>>;",
        );
        let field = ctx.get_field("user", "tags").unwrap();
        assert_eq!(
            field.typ,
            Some(Type::Option(Box::new(Type::Array(
                Box::new(Type::String),
                None
            ))))
        );
    }

    #[test]
    fn build_full_table_type() {
        let ctx = ctx_from(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            DEFINE FIELD email ON user TYPE string;
            "#,
        );
        let table_type = ctx.build_table_type("user").unwrap();
        if let Type::Object(fields) = table_type {
            assert_eq!(fields.len(), 3);
            assert_eq!(fields["name"], Type::String);
            assert_eq!(fields["age"], Type::Int);
            assert_eq!(fields["email"], Type::String);
        } else {
            panic!("Expected object type, got {:?}", table_type);
        }
    }

    #[test]
    fn readonly_and_computed_fields() {
        let ctx = ctx_from(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD id ON user TYPE string READONLY;
            "#,
        );
        let field = ctx.get_field("user", "id").unwrap();
        assert!(field.readonly);
    }
}
