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
    pub indexes: BTreeMap<String, IndexDef>,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexDef {
    pub name: String,
    pub table: String,
    fields: Vec<IndexFieldDef>,
    pub name_span: SourceSpan,
    pub table_span: SourceSpan,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct IndexFieldDef {
    path: Vec<String>,
    text: String,
    span: SourceSpan,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EventDef {
    name: String,
    table: String,
    field_refs: Vec<EventFieldRef>,
    table_span: SourceSpan,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EventFieldRef {
    path: Vec<String>,
    text: String,
    span: SourceSpan,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct IndexTargetRef {
    statement: String,
    index: String,
    table: String,
    index_span: SourceSpan,
    table_span: SourceSpan,
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
    let mut indexes = Vec::new();
    let mut events = Vec::new();
    let mut index_targets = Vec::new();

    for parsed in parsed_sources {
        let root = parsed.tree().root_node();
        collect_definitions(
            root,
            parsed,
            &mut schema,
            &mut fields,
            &mut indexes,
            &mut events,
            &mut index_targets,
            &mut diagnostics,
        );
    }

    for field in fields {
        if let Some(diagnostic) = unsupported_type_diagnostic(&field) {
            diagnostics.push(diagnostic);
        }
        if let Some(diagnostic) = schema.insert_field(field) {
            diagnostics.push(diagnostic);
        }
    }
    validate_indexes(&mut schema, indexes, &mut diagnostics);
    validate_events(&schema, events, &mut diagnostics);
    validate_index_targets(&schema, index_targets, &mut diagnostics);

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
    indexes: &mut Vec<IndexDef>,
    events: &mut Vec<EventDef>,
    index_targets: &mut Vec<IndexTargetRef>,
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
        if let Some(index) = extract_index_def(node, parsed) {
            indexes.push(index);
        }
        if let Some(event) = extract_event_def(node, parsed) {
            events.push(event);
        }
    }

    if let Some(target) = extract_index_target_ref(node, parsed) {
        index_targets.push(target);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_definitions(
            child,
            parsed,
            schema,
            fields,
            indexes,
            events,
            index_targets,
            diagnostics,
        );
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
                indexes: BTreeMap::new(),
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

fn validate_indexes(
    schema: &mut SchemaIndex,
    indexes: Vec<IndexDef>,
    diagnostics: &mut Vec<Finding>,
) {
    for index in indexes {
        let Some(table) = schema.tables.get_mut(&index.table) else {
            diagnostics.push(Finding::new(
                index.table_span.clone(),
                FindingCode::schema(1002),
                Severity::Error,
                format!(
                    "index `{}` targets unknown table `{}`",
                    index.name, index.table
                ),
            ));
            continue;
        };

        for field in &index.fields {
            if !index_field_path_exists_on_table(table, &field.path) {
                diagnostics.push(Finding::new(
                    field.span.clone(),
                    FindingCode::schema(1004),
                    Severity::Error,
                    format!(
                        "index `{}` references unknown field `{}` on table `{}`",
                        index.name, field.text, index.table
                    ),
                ));
            }
        }
        table.indexes.insert(index.name.clone(), index);
    }
}

fn index_field_path_exists_on_table(table: &TableDef, path: &[String]) -> bool {
    let key = path.join(".");
    table.fields.contains_key(&key)
        || table
            .fields
            .values()
            .any(|field| field.path.len() > path.len() && field.path.starts_with(path))
}

fn extract_index_def(node: Node<'_>, parsed: &ParsedSource) -> Option<IndexDef> {
    let mut saw_define = false;
    let mut saw_index = false;
    let mut index_node = None;
    let mut table_node = None;
    let mut fields = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        let text = node_text(child, parsed.text());
        let lower = text.to_ascii_lowercase();

        if child.kind() == "Keyword" {
            if !saw_define && lower == "define" {
                saw_define = true;
                continue;
            }
            if saw_define && !saw_index && lower == "index" {
                saw_index = true;
                continue;
            }
        }

        if saw_define && saw_index && index_node.is_none() && is_identifier_like(child) {
            index_node = Some(child);
            continue;
        }

        if saw_define && saw_index && table_node.is_none() && child.kind() == "OnTableClause" {
            table_node = first_identifier_descendant(child, parsed.text());
            continue;
        }

        if saw_define && saw_index && child.kind() == "FieldsColumnsClause" {
            fields.extend(index_fields_from_clause(child, parsed));
        }
    }

    let index_node = index_node?;
    let table_node = table_node?;
    let index_name = node_text(index_node, parsed.text()).trim().to_string();
    let table_name = node_text(table_node, parsed.text()).trim().to_string();

    Some(IndexDef {
        name: index_name,
        table: table_name,
        fields,
        name_span: node_span(index_node, parsed.source_id().clone()),
        table_span: node_span(table_node, parsed.source_id().clone()),
    })
}

fn index_fields_from_clause(node: Node<'_>, parsed: &ParsedSource) -> Vec<IndexFieldDef> {
    let mut fields = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "Idiom" {
            let text = node_text(child, parsed.text()).trim().to_string();
            fields.push(IndexFieldDef {
                path: text.split('.').map(str::to_string).collect(),
                text,
                span: node_span(child, parsed.source_id().clone()),
            });
        }
    }
    fields
}

fn validate_index_targets(
    schema: &SchemaIndex,
    targets: Vec<IndexTargetRef>,
    diagnostics: &mut Vec<Finding>,
) {
    for target in targets {
        let Some(table) = schema.tables.get(&target.table) else {
            diagnostics.push(Finding::new(
                target.table_span,
                FindingCode::schema(1002),
                Severity::Error,
                format!(
                    "index `{}` targets unknown table `{}` in {} statement",
                    target.index, target.table, target.statement
                ),
            ));
            continue;
        };

        if !table.indexes.contains_key(&target.index) {
            diagnostics.push(Finding::new(
                target.index_span,
                FindingCode::schema(1005),
                Severity::Error,
                format!(
                    "unknown index `{}` on table `{}` in {} statement",
                    target.index, target.table, target.statement
                ),
            ));
        }
    }
}

fn extract_index_target_ref(node: Node<'_>, parsed: &ParsedSource) -> Option<IndexTargetRef> {
    let statement = match node.kind() {
        "RebuildStatement" => "REBUILD",
        "RemoveStatement" => "REMOVE",
        _ => return None,
    };

    let mut saw_statement = false;
    let mut saw_index = false;
    let mut index_node = None;
    let mut table_node = None;
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        let text = node_text(child, parsed.text());
        let upper = text.to_ascii_uppercase();

        if child.kind() == "Keyword" {
            if !saw_statement && upper == statement {
                saw_statement = true;
                continue;
            }
            if saw_statement && !saw_index && upper == "INDEX" {
                saw_index = true;
                continue;
            }
        }

        if saw_statement && saw_index && index_node.is_none() && is_identifier_like(child) {
            index_node = Some(child);
            continue;
        }

        if saw_statement && saw_index && table_node.is_none() && child.kind() == "OnTableClause" {
            table_node = first_identifier_descendant(child, parsed.text());
        }
    }

    let index_node = index_node?;
    let table_node = table_node?;
    Some(IndexTargetRef {
        statement: statement.to_string(),
        index: node_text(index_node, parsed.text()).trim().to_string(),
        table: node_text(table_node, parsed.text()).trim().to_string(),
        index_span: node_span(index_node, parsed.source_id().clone()),
        table_span: node_span(table_node, parsed.source_id().clone()),
    })
}

fn validate_events(schema: &SchemaIndex, events: Vec<EventDef>, diagnostics: &mut Vec<Finding>) {
    for event in events {
        let Some(table) = schema.tables.get(&event.table) else {
            diagnostics.push(Finding::new(
                event.table_span,
                FindingCode::schema(1002),
                Severity::Error,
                format!(
                    "event `{}` targets unknown table `{}`",
                    event.name, event.table
                ),
            ));
            continue;
        };

        for field_ref in event.field_refs {
            if !index_field_path_exists_on_table(table, &field_ref.path) {
                diagnostics.push(Finding::new(
                    field_ref.span,
                    FindingCode::schema(1004),
                    Severity::Error,
                    format!(
                        "event `{}` references unknown field `{}` on table `{}`",
                        event.name, field_ref.text, event.table
                    ),
                ));
            }
        }
    }
}

fn extract_event_def(node: Node<'_>, parsed: &ParsedSource) -> Option<EventDef> {
    let mut saw_define = false;
    let mut saw_event = false;
    let mut event_node = None;
    let mut table_node = None;
    let mut field_refs = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        let text = node_text(child, parsed.text());
        let lower = text.to_ascii_lowercase();

        if child.kind() == "Keyword" {
            if !saw_define && lower == "define" {
                saw_define = true;
                continue;
            }
            if saw_define && !saw_event && lower == "event" {
                saw_event = true;
                continue;
            }
        }

        if saw_define && saw_event && event_node.is_none() && is_identifier_like(child) {
            event_node = Some(child);
            continue;
        }

        if saw_define && saw_event && table_node.is_none() && child.kind() == "OnTableClause" {
            table_node = first_identifier_descendant(child, parsed.text());
            continue;
        }

        if saw_define && saw_event && matches!(child.kind(), "WhenClause" | "ThenClause") {
            collect_event_field_refs(child, parsed, &mut field_refs);
        }
    }

    let event_node = event_node?;
    let table_node = table_node?;
    Some(EventDef {
        name: node_text(event_node, parsed.text()).trim().to_string(),
        table: node_text(table_node, parsed.text()).trim().to_string(),
        field_refs,
        table_span: node_span(table_node, parsed.source_id().clone()),
    })
}

fn collect_event_field_refs(
    node: Node<'_>,
    parsed: &ParsedSource,
    field_refs: &mut Vec<EventFieldRef>,
) {
    if node.kind() == "Path" {
        let text = node_text(node, parsed.text()).trim();
        if let Some(field_text) = text.strip_prefix("$event.") {
            field_refs.push(EventFieldRef {
                path: field_text.split('.').map(str::to_string).collect(),
                text: field_text.to_string(),
                span: node_span(node, parsed.source_id().clone()),
            });
            return;
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_event_field_refs(child, parsed, field_refs);
    }
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
