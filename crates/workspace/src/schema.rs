use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use surrealguard_diagnostics::{Finding, FindingCode, Severity};
use surrealguard_syntax::parse::ParsedSource;
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};
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
}

pub fn extract_schema(parsed_sources: &[ParsedSource]) -> SchemaExtraction {
    let mut schema = SchemaIndex::default();
    let mut diagnostics = Vec::new();

    for parsed in parsed_sources {
        let root = parsed.tree().root_node();
        collect_tables(root, parsed, &mut schema, &mut diagnostics);
    }

    SchemaExtraction {
        schema,
        diagnostics,
    }
}

fn collect_tables(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &mut SchemaIndex,
    diagnostics: &mut Vec<Finding>,
) {
    if node.kind() == "DefineStatement" {
        if let Some(table) = extract_table_def(node, parsed) {
            if let Some(diagnostic) = schema.insert_table(table) {
                diagnostics.push(diagnostic);
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_tables(child, parsed, schema, diagnostics);
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
            });
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
