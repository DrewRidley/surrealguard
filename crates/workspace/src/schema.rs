use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use surrealdb_types::Kind;
use surrealguard_diagnostics::{Finding, FindingCode, Severity};
use surrealguard_syntax::parse::ParsedSource;
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use crate::response_shape::PartialReason;
use tree_sitter::Node;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaIndex {
    pub tables: BTreeMap<String, TableDef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableDef {
    pub name: String,
    pub source: SourceId,
    pub name_span: SourceSpan,
    pub fields: BTreeMap<String, FieldDef>,
    pub relation: Option<RelationDef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationDef {
    pub in_tables: Vec<String>,
    pub out_tables: Vec<String>,
    pub span: SourceSpan,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDef {
    pub path: Vec<String>,
    pub table: String,
    pub kind: Option<Kind>,
    pub partial: Vec<PartialReason>,
    pub source: SourceId,
    pub name_span: SourceSpan,
    pub table_span: SourceSpan,
    pub type_span: Option<SourceSpan>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaExtraction {
    pub schema: SchemaIndex,
    pub diagnostics: Vec<Finding>,
}

impl SchemaIndex {
    pub fn insert_table(&mut self, table: TableDef) -> Option<Finding> {
        if self.tables.contains_key(&table.name) {
            return Some(Finding::new(
                table.name_span,
                FindingCode::schema(1001),
                Severity::Error,
                format!("duplicate table definition `{}`", table.name),
            ));
        }

        self.tables.insert(table.name.clone(), table);
        None
    }

    pub fn insert_field(&mut self, field: FieldDef) -> Option<Finding> {
        let field_key = field.path.join(".");
        let table_name = field.table.clone();
        let table_span = field.table_span.clone();
        let Some(table) = self.tables.get_mut(&table_name) else {
            return Some(Finding::new(
                table_span,
                FindingCode::schema(1002),
                Severity::Error,
                format!(
                    "field `{}` targets unknown table `{}`",
                    field_key, table_name
                ),
            ));
        };

        table.fields.insert(field_key, field);
        None
    }
}

pub fn extract_schema(parsed_sources: &[ParsedSource]) -> SchemaExtraction {
    let mut schema = SchemaIndex::default();
    let mut diagnostics = Vec::new();
    let mut fields = Vec::new();

    for parsed in parsed_sources {
        let root = parsed.tree().root_node();
        collect_definitions(root, parsed, &mut schema, &mut fields, &mut diagnostics);
    }

    for field in fields {
        if let Some(diagnostic) = unsupported_type_diagnostic(&field) {
            diagnostics.push(diagnostic);
        }
        if let Some(diagnostic) = schema.insert_field(field) {
            diagnostics.push(diagnostic);
        }
    }

    SchemaExtraction {
        schema,
        diagnostics,
    }
}

fn collect_definitions(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &mut SchemaIndex,
    fields: &mut Vec<FieldDef>,
    diagnostics: &mut Vec<Finding>,
) {
    if node.kind() == "DefineStatement" {
        if let Some(table) = extract_table_def(node, parsed) {
            if let Some(diagnostic) = schema.insert_table(table) {
                diagnostics.push(diagnostic);
            }
        }
        if let Some(field) = extract_field_def(node, parsed) {
            fields.push(field);
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_definitions(child, parsed, schema, fields, diagnostics);
    }
}

fn extract_table_def(node: Node<'_>, parsed: &ParsedSource) -> Option<TableDef> {
    let mut saw_define = false;
    let mut saw_table = false;
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        let text = node_text(child, parsed.text());
        let lower = text.to_ascii_lowercase();

        if child.kind() == "Keyword" {
            if !saw_define && lower == "define" {
                saw_define = true;
                continue;
            }
            if saw_define && !saw_table && lower == "table" {
                saw_table = true;
                continue;
            }
        }

        if saw_define && saw_table && is_identifier_like(child) {
            return Some(TableDef {
                name: text.to_string(),
                source: parsed.source_id().clone(),
                name_span: node_span(child, parsed.source_id().clone()),
                fields: BTreeMap::new(),
                relation: relation_def_from_define_table(node, parsed),
            });
        }
    }

    None
}

fn relation_def_from_define_table(node: Node<'_>, parsed: &ParsedSource) -> Option<RelationDef> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "TableTypeClause" {
            continue;
        }

        let mut is_relation = false;
        let mut current_side = None;
        let mut in_tables = Vec::new();
        let mut out_tables = Vec::new();
        let mut clause_cursor = child.walk();

        for clause_child in child.children(&mut clause_cursor) {
            let text = node_text(clause_child, parsed.text()).trim();
            let upper = text.to_ascii_uppercase();
            if clause_child.kind() == "Keyword" {
                match upper.as_str() {
                    "RELATION" => is_relation = true,
                    "IN" => current_side = Some("in"),
                    "OUT" => current_side = Some("out"),
                    _ => {}
                }
                continue;
            }

            if is_identifier_like(clause_child) {
                match current_side {
                    Some("in") => in_tables.push(text.to_string()),
                    Some("out") => out_tables.push(text.to_string()),
                    _ => {}
                }
            }
        }

        if is_relation {
            return Some(RelationDef {
                in_tables,
                out_tables,
                span: node_span(child, parsed.source_id().clone()),
            });
        }
    }

    None
}

fn extract_field_def(node: Node<'_>, parsed: &ParsedSource) -> Option<FieldDef> {
    let mut saw_define = false;
    let mut saw_field = false;
    let mut field_node = None;
    let mut table_node = None;
    let mut type_node = None;
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        let text = node_text(child, parsed.text());
        let lower = text.to_ascii_lowercase();

        if child.kind() == "Keyword" {
            if !saw_define && lower == "define" {
                saw_define = true;
                continue;
            }
            if saw_define && !saw_field && lower == "field" {
                saw_field = true;
                continue;
            }
        }

        if saw_define && saw_field && field_node.is_none() && child.kind() == "Idiom" {
            field_node = Some(child);
            continue;
        }

        if saw_define && saw_field && table_node.is_none() && child.kind() == "OnTableClause" {
            table_node = first_identifier_descendant(child, parsed.text());
            continue;
        }

        if saw_define && saw_field && type_node.is_none() && child.kind() == "TypeClause" {
            type_node = type_descendant(child, parsed.text());
        }
    }

    let field_node = field_node?;
    let table_node = table_node?;
    let field_text = node_text(field_node, parsed.text()).trim();
    let table_text = node_text(table_node, parsed.text()).trim();
    let (kind, partial, type_span) = match type_node {
        Some(type_node) => {
            let type_text = node_text(type_node, parsed.text()).trim();
            let parsed_type = parse_field_kind(type_text);
            (
                parsed_type.kind,
                parsed_type.partial,
                Some(node_span(type_node, parsed.source_id().clone())),
            )
        }
        None => (None, vec![PartialReason::Unresolved], None),
    };

    Some(FieldDef {
        path: field_text.split('.').map(str::to_string).collect(),
        table: table_text.to_string(),
        kind,
        partial,
        source: parsed.source_id().clone(),
        name_span: node_span(field_node, parsed.source_id().clone()),
        table_span: node_span(table_node, parsed.source_id().clone()),
        type_span,
    })
}

fn unsupported_type_diagnostic(field: &FieldDef) -> Option<Finding> {
    let type_text = field.partial.iter().find_map(|reason| match reason {
        PartialReason::UnsupportedSyntax(type_text) => Some(type_text),
        PartialReason::Unresolved | PartialReason::DynamicExpression => None,
    })?;

    Some(Finding::new(
        field
            .type_span
            .clone()
            .unwrap_or_else(|| field.name_span.clone()),
        FindingCode::dynamic(6001),
        Severity::Warning,
        format!(
            "unsupported field type syntax `{}` for field `{}`",
            type_text,
            field.path.join(".")
        ),
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ParsedFieldKind {
    kind: Option<Kind>,
    partial: Vec<PartialReason>,
}

fn parse_field_kind(type_text: &str) -> ParsedFieldKind {
    let kind = match type_text.to_ascii_lowercase().as_str() {
        "any" => Some(Kind::Any),
        "none" => Some(Kind::None),
        "null" => Some(Kind::Null),
        "bool" | "boolean" => Some(Kind::Bool),
        "string" => Some(Kind::String),
        "number" => Some(Kind::Number),
        "int" => Some(Kind::Int),
        "float" => Some(Kind::Float),
        "decimal" => Some(Kind::Decimal),
        "datetime" => Some(Kind::Datetime),
        "duration" => Some(Kind::Duration),
        "uuid" => Some(Kind::Uuid),
        "bytes" => Some(Kind::Bytes),
        "record" => Some(Kind::Record(Vec::new())),
        _ => None,
    };

    match kind {
        Some(kind) => ParsedFieldKind {
            kind: Some(kind),
            partial: Vec::new(),
        },
        None => ParsedFieldKind {
            kind: None,
            partial: vec![PartialReason::UnsupportedSyntax(type_text.to_string())],
        },
    }
}

fn first_identifier_descendant<'tree>(node: Node<'tree>, source: &str) -> Option<Node<'tree>> {
    if is_identifier_like(node) {
        return Some(node);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let text = node_text(child, source).to_ascii_lowercase();
        if child.kind() == "Keyword" || text == "on" || text == "table" {
            continue;
        }
        if let Some(identifier) = first_identifier_descendant(child, source) {
            return Some(identifier);
        }
    }

    None
}

fn type_descendant<'tree>(node: Node<'tree>, source: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let text = node_text(child, source).to_ascii_lowercase();
        if child.kind() == "Keyword" || text == "type" || text == "flexible" {
            continue;
        }
        if matches!(
            child.kind(),
            "TypeName" | "ParameterizedType" | "UnionType" | "ArrayType" | "ObjectType"
        ) {
            return Some(child);
        }
        if let Some(found) = type_descendant(child, source) {
            return Some(found);
        }
    }

    None
}

fn is_identifier_like(node: Node<'_>) -> bool {
    matches!(node.kind(), "Ident" | "RecordId" | "Thing" | "Identifier")
}

fn node_text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    &source[node.start_byte()..node.end_byte()]
}

fn node_span(node: Node<'_>, source: SourceId) -> SourceSpan {
    let start = node.start_byte().min(u32::MAX as usize) as u32;
    let end = node.end_byte().min(u32::MAX as usize) as u32;
    SourceSpan::new(
        source,
        ByteRange::new(start, end).expect("tree-sitter node byte ranges are ordered"),
    )
}
