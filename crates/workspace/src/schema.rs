//! Schema extraction: walks `DEFINE`/`REMOVE`/`ALTER` statements into the
//! `SchemaIndex` (tables, fields, params, functions, analyzers), converts
//! declared type syntax to upstream `Kind`s, and validates definition
//! references. Statement walking is still node-based; type parsing is
//! structural via the lowered `TypeExpr`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use surrealdb_types::Kind;
use surrealguard_diagnostics::{Finding, FindingCode, Severity};
use surrealguard_syntax::parse::ParsedSource;
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use crate::expression::PartialReason;
use tree_sitter::Node;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaIndex {
    pub tables: BTreeMap<String, TableDef>,
    pub params: BTreeMap<String, ParamDef>,
    pub functions: BTreeMap<String, FunctionDef>,
    pub analyzers: BTreeMap<String, AnalyzerDef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamDef {
    pub name: String,
    pub source: SourceId,
    pub name_span: SourceSpan,
    pub value_span: Option<SourceSpan>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    pub args: Vec<String>,
    pub return_kind: Option<Kind>,
    pub source: SourceId,
    pub name_span: SourceSpan,
    pub return_span: Option<SourceSpan>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyzerDef {
    pub name: String,
    pub tokenizers: Vec<String>,
    pub filters: Vec<String>,
    pub source: SourceId,
    pub name_span: SourceSpan,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FieldPath(Vec<String>);

impl FieldPath {
    pub fn new(parts: Vec<String>) -> Self {
        Self(parts)
    }

    pub fn parse(path: &str) -> Self {
        Self(
            path.split('.')
                .filter(|part| !part.is_empty())
                .map(ToString::to_string)
                .collect(),
        )
    }

    pub fn parts(&self) -> &[String] {
        &self.0
    }

    pub fn dotted(&self) -> String {
        self.0.join(".")
    }

    pub fn is_prefix_of(&self, path: &[String]) -> bool {
        self.0.len() <= path.len()
            && self
                .0
                .iter()
                .zip(path.iter())
                .all(|(left, right)| left == right)
    }
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
struct AlterTargetRef {
    table: String,
    table_span: SourceSpan,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RemoveTargetRef {
    Table {
        table: String,
        span: SourceSpan,
    },
    Field {
        field: String,
        path: Vec<String>,
        table: String,
        field_span: SourceSpan,
        table_span: SourceSpan,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaExtraction {
    pub schema: SchemaIndex,
    pub diagnostics: Vec<Finding>,
}

impl SchemaIndex {
    pub fn table(&self, name: &str) -> Option<&TableDef> {
        self.tables.get(name)
    }

    pub fn field(&self, table: &str, path: &FieldPath) -> Option<&FieldDef> {
        self.table(table)?.field(path)
    }

    pub fn param(&self, name: &str) -> Option<&ParamDef> {
        self.params.get(name.strip_prefix('$').unwrap_or(name))
    }

    pub fn function(&self, name: &str) -> Option<&FunctionDef> {
        self.functions.get(name)
    }

    pub fn analyzer(&self, name: &str) -> Option<&AnalyzerDef> {
        self.analyzers.get(name)
    }

    pub fn insert_param(&mut self, param: ParamDef) {
        self.params.insert(param.name.clone(), param);
    }

    pub fn insert_function(&mut self, function: FunctionDef) {
        self.functions.insert(function.name.clone(), function);
    }

    pub fn insert_analyzer(&mut self, analyzer: AnalyzerDef) {
        self.analyzers.insert(analyzer.name.clone(), analyzer);
    }

    pub fn insert_table(&mut self, mut table: TableDef, overwrite: bool) -> Option<Finding> {
        if let Some(existing) = self.tables.remove(&table.name) {
            if !overwrite {
                let name = table.name.clone();
                let span = table.name_span.clone();
                self.tables.insert(existing.name.clone(), existing);
                return Some(Finding::new(
                    span,
                    FindingCode::schema(1001),
                    Severity::Error,
                    format!("duplicate table definition `{name}`"),
                ));
            }

            // SurrealDB overwrites the table definition itself, but existing
            // field/index definitions remain attached to the table.
            table.fields = existing.fields;
            table.indexes = existing.indexes;
        }

        self.tables.insert(table.name.clone(), table);
        None
    }

    pub fn insert_field(&mut self, field: FieldDef, overwrite: bool) -> Option<Finding> {
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

        if table.fields.contains_key(&field_key) && !overwrite {
            return Some(Finding::new(
                field.name_span,
                FindingCode::schema(1001),
                Severity::Error,
                format!("duplicate field definition `{field_key}` on table `{table_name}`"),
            ));
        }

        table.fields.insert(field_key, field);
        None
    }

    pub fn remove_table(&mut self, table: &str, span: SourceSpan) -> Option<Finding> {
        if self.tables.remove(table).is_none() {
            return Some(Finding::new(
                span,
                FindingCode::schema(1002),
                Severity::Error,
                format!("REMOVE TABLE targets unknown table `{table}`"),
            ));
        }
        None
    }

    pub fn remove_field(
        &mut self,
        table: &str,
        field: &str,
        path: &[String],
        field_span: SourceSpan,
        table_span: SourceSpan,
    ) -> Option<Finding> {
        let Some(table_def) = self.tables.get_mut(table) else {
            return Some(Finding::new(
                table_span,
                FindingCode::schema(1002),
                Severity::Error,
                format!("REMOVE FIELD `{field}` targets unknown table `{table}`"),
            ));
        };

        let key = path.join(".");
        if table_def.fields.remove(&key).is_none() {
            return Some(Finding::new(
                field_span,
                FindingCode::schema(1004),
                Severity::Error,
                format!("REMOVE FIELD targets unknown field `{field}` on table `{table}`"),
            ));
        }
        None
    }
}

impl TableDef {
    pub fn field(&self, path: &FieldPath) -> Option<&FieldDef> {
        self.fields.get(&path.dotted())
    }

    pub fn fields_under<'a>(
        &'a self,
        prefix: &'a FieldPath,
    ) -> impl Iterator<Item = &'a FieldDef> + 'a {
        self.fields
            .values()
            .filter(move |field| prefix.is_prefix_of(&field.path))
    }
}

pub(crate) fn apply_schema_statement_effects(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &mut SchemaIndex,
) -> Vec<Finding> {
    let mut diagnostics = Vec::new();
    let mut indexes = Vec::new();
    let mut events = Vec::new();
    let mut index_targets = Vec::new();
    let mut alter_targets = Vec::new();

    collect_definitions(
        node,
        parsed,
        schema,
        &mut indexes,
        &mut events,
        &mut index_targets,
        &mut alter_targets,
        &mut diagnostics,
    );

    validate_indexes(schema, indexes, &mut diagnostics);
    validate_events(schema, events, &mut diagnostics);
    validate_index_targets(schema, index_targets, &mut diagnostics);
    validate_alter_targets(schema, alter_targets, &mut diagnostics);

    diagnostics
}

pub fn extract_schema(parsed_sources: &[ParsedSource]) -> SchemaExtraction {
    let mut schema = SchemaIndex::default();
    let mut diagnostics = Vec::new();
    let mut indexes = Vec::new();
    let mut events = Vec::new();
    let mut index_targets = Vec::new();
    let mut alter_targets = Vec::new();

    for parsed in parsed_sources {
        let root = parsed.tree().root_node();
        collect_definitions(
            root,
            parsed,
            &mut schema,
            &mut indexes,
            &mut events,
            &mut index_targets,
            &mut alter_targets,
            &mut diagnostics,
        );
    }

    validate_indexes(&mut schema, indexes, &mut diagnostics);
    validate_events(&schema, events, &mut diagnostics);
    validate_index_targets(&schema, index_targets, &mut diagnostics);
    validate_alter_targets(&schema, alter_targets, &mut diagnostics);

    SchemaExtraction {
        schema,
        diagnostics,
    }
}

// Node-based walker retained only for the remaining validators; not worth
// a parameter-object refactor before it is replaced.
#[allow(clippy::too_many_arguments)]
fn collect_definitions(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &mut SchemaIndex,
    indexes: &mut Vec<IndexDef>,
    events: &mut Vec<EventDef>,
    index_targets: &mut Vec<IndexTargetRef>,
    alter_targets: &mut Vec<AlterTargetRef>,
    diagnostics: &mut Vec<Finding>,
) {
    if node.kind() == "DefineStatement" {
        let overwrite = define_has_overwrite(node, parsed);
        if let Some(table) = extract_table_def(node, parsed) {
            if let Some(diagnostic) = schema.insert_table(table, overwrite) {
                diagnostics.push(diagnostic);
            }
        }
        if let Some(field) = extract_field_def(node, parsed) {
            if let Some(diagnostic) = unsupported_type_diagnostic(&field) {
                diagnostics.push(diagnostic);
            }
            if let Some(diagnostic) = schema.insert_field(field, overwrite) {
                diagnostics.push(diagnostic);
            }
        }
        if let Some(index) = extract_index_def(node, parsed) {
            indexes.push(index);
        }
        if let Some(event) = extract_event_def(node, parsed) {
            events.push(event);
        }
        if let Some(param) = extract_param_def(node, parsed) {
            schema.insert_param(param);
        }
        if let Some(function) = extract_function_def(node, parsed) {
            schema.insert_function(function);
        }
        if let Some(analyzer) = extract_analyzer_def(node, parsed) {
            schema.insert_analyzer(analyzer);
        }
    }

    if let Some(target) = extract_index_target_ref(node, parsed) {
        index_targets.push(target);
    }

    if let Some(target) = extract_alter_target_ref(node, parsed) {
        alter_targets.push(target);
    }

    if let Some(target) = extract_remove_target_ref(node, parsed) {
        if let Some(diagnostic) = apply_remove_target(schema, target) {
            diagnostics.push(diagnostic);
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_definitions(
            child,
            parsed,
            schema,
            indexes,
            events,
            index_targets,
            alter_targets,
            diagnostics,
        );
    }
}

fn define_has_overwrite(node: Node<'_>, parsed: &ParsedSource) -> bool {
    let statement = node_text(node, parsed.text());
    statement
        .split(|ch: char| !ch.is_ascii_alphabetic())
        .any(|word| word.eq_ignore_ascii_case("overwrite"))
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
            let ty = surrealguard_syntax::lower::lower_type_expr(type_node, parsed.text());
            let parsed_type = kind_from_type_expr(&ty.node, parsed.text());
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

fn extract_param_def(node: Node<'_>, parsed: &ParsedSource) -> Option<ParamDef> {
    let statement = node_text(node, parsed.text());
    if !statement
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("define param")
    {
        return None;
    }

    let sigil_start = statement.find('$')?;
    let name_start = sigil_start + 1;
    let name_end = name_start
        + statement[name_start..]
            .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
            .unwrap_or(statement.len() - name_start);
    if name_end == name_start {
        return None;
    }

    let value_span = statement
        .to_ascii_lowercase()
        .find(" value ")
        .map(|relative| {
            span_from_relative_range(
                node,
                parsed.source_id().clone(),
                relative + 1,
                statement.len(),
            )
        });

    Some(ParamDef {
        name: statement[name_start..name_end].to_string(),
        source: parsed.source_id().clone(),
        name_span: span_from_relative_range(
            node,
            parsed.source_id().clone(),
            sigil_start,
            name_end,
        ),
        value_span,
    })
}

fn extract_function_def(node: Node<'_>, parsed: &ParsedSource) -> Option<FunctionDef> {
    let statement = node_text(node, parsed.text());
    if !statement
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("define function")
    {
        return None;
    }

    let name_start = statement.find("fn::")?;
    let name_end = name_start
        + statement[name_start..]
            .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == ':' || ch == '-'))
            .unwrap_or(statement.len() - name_start);
    let name = statement[name_start..name_end].to_string();

    let args = statement[name_end..]
        .find('(')
        .and_then(|open| {
            let open = name_end + open;
            statement[open + 1..]
                .find(')')
                .map(|close| (open + 1, open + 1 + close))
        })
        .map(|(open, close)| parse_function_arg_names(&statement[open..close]))
        .unwrap_or_default();

    let (return_kind, return_span) = statement.find("->").map_or((None, None), |arrow| {
        let type_start = arrow
            + 2
            + statement[arrow + 2..]
                .chars()
                .take_while(|ch| ch.is_whitespace())
                .map(char::len_utf8)
                .sum::<usize>();
        let type_end = type_start
            + statement[type_start..]
                .find(|ch: char| ch.is_whitespace() || ch == '{' || ch == ';')
                .unwrap_or(statement.len() - type_start);
        let type_text = statement[type_start..type_end].trim();
        let parsed_type = parse_field_kind(type_text);
        (
            parsed_type.kind,
            Some(span_from_relative_range(
                node,
                parsed.source_id().clone(),
                type_start,
                type_end,
            )),
        )
    });

    Some(FunctionDef {
        name,
        args,
        return_kind,
        source: parsed.source_id().clone(),
        name_span: span_from_relative_range(node, parsed.source_id().clone(), name_start, name_end),
        return_span,
    })
}

fn extract_analyzer_def(node: Node<'_>, parsed: &ParsedSource) -> Option<AnalyzerDef> {
    let statement = node_text(node, parsed.text());
    let trimmed = statement.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("define analyzer") {
        return None;
    }

    let statement_offset = statement.len() - trimmed.len();
    let name_relative_start = statement_offset + "define analyzer".len();
    let name_start = name_relative_start
        + statement[name_relative_start..]
            .chars()
            .take_while(|ch| ch.is_whitespace())
            .map(char::len_utf8)
            .sum::<usize>();
    let name_end = name_start
        + statement[name_start..]
            .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
            .unwrap_or(statement.len() - name_start);
    if name_end == name_start {
        return None;
    }

    Some(AnalyzerDef {
        name: statement[name_start..name_end].to_string(),
        tokenizers: parse_analyzer_list_after_keyword(
            statement,
            "tokenizers",
            &["filters", "comments"],
        ),
        filters: parse_analyzer_list_after_keyword(
            statement,
            "filters",
            &["tokenizers", "comments"],
        ),
        source: parsed.source_id().clone(),
        name_span: span_from_relative_range(node, parsed.source_id().clone(), name_start, name_end),
    })
}

fn parse_analyzer_list_after_keyword(
    statement: &str,
    keyword: &str,
    terminators: &[&str],
) -> Vec<String> {
    let lower = statement.to_ascii_lowercase();
    let Some(keyword_start) = lower.find(keyword) else {
        return Vec::new();
    };
    let list_start = keyword_start + keyword.len();
    let mut list_end = statement.len();

    for terminator in terminators {
        if let Some(relative) = lower[list_start..].find(terminator) {
            list_end = list_end.min(list_start + relative);
        }
    }

    statement[list_start..list_end]
        .trim_matches(|ch: char| ch.is_whitespace() || ch == ';')
        .split(',')
        .flat_map(|chunk| chunk.split_whitespace())
        .map(|entry| entry.trim_matches(';').to_string())
        .filter(|entry| !entry.is_empty())
        .collect()
}

fn parse_function_arg_names(args_text: &str) -> Vec<String> {
    args_text
        .split(',')
        .filter_map(|arg| {
            let arg = arg.trim();
            let name = arg.strip_prefix('$')?;
            let end = name
                .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                .unwrap_or(name.len());
            (end > 0).then(|| name[..end].to_string())
        })
        .collect()
}

fn span_from_relative_range(
    node: Node<'_>,
    source: SourceId,
    relative_start: usize,
    relative_end: usize,
) -> SourceSpan {
    let start = node
        .start_byte()
        .saturating_add(relative_start)
        .min(u32::MAX as usize) as u32;
    let end = node
        .start_byte()
        .saturating_add(relative_end)
        .min(u32::MAX as usize) as u32;
    SourceSpan::new(
        source,
        ByteRange::new(start, end).expect("relative byte ranges are ordered"),
    )
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

fn validate_alter_targets(
    schema: &SchemaIndex,
    targets: Vec<AlterTargetRef>,
    diagnostics: &mut Vec<Finding>,
) {
    for target in targets {
        if !schema.tables.contains_key(&target.table) {
            diagnostics.push(Finding::new(
                target.table_span,
                FindingCode::schema(1002),
                Severity::Error,
                format!("ALTER TABLE targets unknown table `{}`", target.table),
            ));
        }
    }
}

fn extract_alter_target_ref(node: Node<'_>, parsed: &ParsedSource) -> Option<AlterTargetRef> {
    if node.kind() != "AlterStatement" {
        return None;
    }

    let mut saw_alter = false;
    let mut saw_table = false;
    let mut table_node = None;
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        let text = node_text(child, parsed.text());
        let upper = text.to_ascii_uppercase();

        if child.kind() == "Keyword" {
            if !saw_alter && upper == "ALTER" {
                saw_alter = true;
                continue;
            }
            if saw_alter && !saw_table && upper == "TABLE" {
                saw_table = true;
                continue;
            }
        }

        if saw_alter && saw_table && table_node.is_none() && is_identifier_like(child) {
            table_node = Some(child);
        }
    }

    let table_node = table_node?;
    Some(AlterTargetRef {
        table: node_text(table_node, parsed.text()).trim().to_string(),
        table_span: node_span(table_node, parsed.source_id().clone()),
    })
}

fn apply_remove_target(schema: &mut SchemaIndex, target: RemoveTargetRef) -> Option<Finding> {
    match target {
        RemoveTargetRef::Table { table, span } => schema.remove_table(&table, span),
        RemoveTargetRef::Field {
            field,
            path,
            table,
            field_span,
            table_span,
        } => schema.remove_field(&table, &field, &path, field_span, table_span),
    }
}

fn extract_remove_target_ref(node: Node<'_>, parsed: &ParsedSource) -> Option<RemoveTargetRef> {
    if node.kind() != "RemoveStatement" {
        return None;
    }

    let mut saw_remove = false;
    let mut target_kind: Option<&str> = None;
    let mut object_node = None;
    let mut table_node = None;
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        let text = node_text(child, parsed.text());
        let upper = text.to_ascii_uppercase();

        if child.kind() == "Keyword" {
            if !saw_remove && upper == "REMOVE" {
                saw_remove = true;
                continue;
            }
            if saw_remove && target_kind.is_none() && matches!(upper.as_str(), "TABLE" | "FIELD") {
                target_kind = Some(if upper == "TABLE" { "TABLE" } else { "FIELD" });
                continue;
            }
        }

        if saw_remove
            && target_kind.is_some()
            && object_node.is_none()
            && (is_identifier_like(child) || child.kind() == "Path")
        {
            object_node = Some(child);
            continue;
        }

        if saw_remove && target_kind == Some("FIELD") && child.kind() == "OnTableClause" {
            table_node = first_identifier_descendant(child, parsed.text());
        }
    }

    match target_kind? {
        "TABLE" => {
            let object_node = object_node?;
            Some(RemoveTargetRef::Table {
                table: node_text(object_node, parsed.text()).trim().to_string(),
                span: node_span(object_node, parsed.source_id().clone()),
            })
        }
        "FIELD" => {
            let field_node = object_node?;
            let table_node = table_node?;
            let field = node_text(field_node, parsed.text()).trim().to_string();
            Some(RemoveTargetRef::Field {
                path: field.split('.').map(str::to_string).collect(),
                field,
                table: node_text(table_node, parsed.text()).trim().to_string(),
                field_span: node_span(field_node, parsed.source_id().clone()),
                table_span: node_span(table_node, parsed.source_id().clone()),
            })
        }
        _ => None,
    }
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
pub(crate) struct ParsedFieldKind {
    pub(crate) kind: Option<Kind>,
    pub(crate) partial: Vec<PartialReason>,
}

/// Converts a structurally lowered type to an upstream `Kind`:
/// `array<string>`, `option<int>`, unions, and literal types all resolve.
/// Anything the conversion can't express reports why as an explicit
/// partial reason.
pub(crate) fn kind_from_type_expr(
    ty: &surrealguard_syntax::ast::TypeExpr,
    text: &str,
) -> ParsedFieldKind {
    use surrealguard_syntax::ast::TypeExpr;

    fn convert(ty: &TypeExpr, text: &str) -> Result<Kind, PartialReason> {
        match ty {
            TypeExpr::Name(name) => base_kind_for_name(&name.node)
                .ok_or_else(|| PartialReason::UnsupportedSyntax(name.node.clone())),
            TypeExpr::Parameterized { name, args } => parameterized_kind(&name.node, args, text),
            TypeExpr::Union(variants) => {
                let kinds = variants
                    .iter()
                    .map(|variant| convert(&variant.node, text))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Kind::either(kinds))
            }
            TypeExpr::Optional(inner) => {
                let inner = convert(&inner.node, text)?;
                Ok(Kind::either(vec![Kind::None, inner]))
            }
            TypeExpr::Literal(literal) => literal_kind(literal),
            TypeExpr::Partial(partial) => {
                let start = partial.span.start() as usize;
                let end = partial.span.end() as usize;
                let source = text
                    .get(start..end)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| partial.cst_kind.clone());
                Err(PartialReason::UnsupportedSyntax(source))
            }
        }
    }

    fn parameterized_kind(
        name: &str,
        args: &[surrealguard_syntax::ast::Spanned<TypeExpr>],
        text: &str,
    ) -> Result<Kind, PartialReason> {
        let unsupported = || PartialReason::UnsupportedSyntax(format!("{name}<...>"));
        match name.to_ascii_lowercase().as_str() {
            "record" => {
                let tables = args
                    .iter()
                    .filter_map(|arg| match &arg.node {
                        TypeExpr::Name(table) => {
                            Some(surrealdb_types::Table::from(table.node.as_str()))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if tables.len() == args.len() && !tables.is_empty() {
                    Ok(Kind::Record(tables))
                } else {
                    Err(unsupported())
                }
            }
            "array" | "set" => {
                let mut element = Kind::Any;
                let mut max_len = None;
                for arg in args {
                    match &arg.node {
                        TypeExpr::Literal(surrealguard_syntax::ast::Literal::Int(len)) => {
                            max_len = u64::try_from(*len).ok();
                        }
                        other => element = convert(other, text)?,
                    }
                }
                if name.eq_ignore_ascii_case("set") {
                    Ok(Kind::Set(Box::new(element), max_len))
                } else {
                    Ok(Kind::Array(Box::new(element), max_len))
                }
            }
            _ => Err(unsupported()),
        }
    }

    fn literal_kind(literal: &surrealguard_syntax::ast::Literal) -> Result<Kind, PartialReason> {
        use surrealdb_types::KindLiteral;
        use surrealguard_syntax::ast::Literal;
        let kind = match literal {
            Literal::String(value) => KindLiteral::String(value.clone()),
            Literal::Int(value) => KindLiteral::Integer(*value),
            Literal::Float(value) => KindLiteral::Float(*value),
            Literal::Bool(value) => KindLiteral::Bool(*value),
            _ => {
                return Err(PartialReason::UnsupportedSyntax("literal type".into()));
            }
        };
        Ok(Kind::Literal(kind))
    }

    match convert(ty, text) {
        Ok(kind) => ParsedFieldKind {
            kind: Some(kind),
            partial: Vec::new(),
        },
        Err(reason) => ParsedFieldKind {
            kind: None,
            partial: vec![reason],
        },
    }
}

fn base_kind_for_name(name: &str) -> Option<Kind> {
    let kind = match name.to_ascii_lowercase().as_str() {
        "any" => Kind::Any,
        "none" => Kind::None,
        "null" => Kind::Null,
        "bool" | "boolean" => Kind::Bool,
        "string" => Kind::String,
        "number" => Kind::Number,
        "int" => Kind::Int,
        "float" => Kind::Float,
        "decimal" => Kind::Decimal,
        "datetime" => Kind::Datetime,
        "duration" => Kind::Duration,
        "uuid" => Kind::Uuid,
        "bytes" => Kind::Bytes,
        "object" => Kind::Object,
        "array" => Kind::Array(Box::new(Kind::Any), None),
        "set" => Kind::Set(Box::new(Kind::Any), None),
        "record" => Kind::Record(Vec::new()),
        "geometry" => Kind::Geometry(Vec::new()),
        _ => return None,
    };
    Some(kind)
}

fn parse_field_kind(type_text: &str) -> ParsedFieldKind {
    let normalized = type_text.to_ascii_lowercase();
    // `record<person>` / `record<person | company>` — the one parameterized
    // form this text parser supports. Structured types go through
    // `kind_from_type_expr` instead; this path serves the remaining
    // text-sliced callers (function parameter types).
    if let Some(targets) = normalized
        .strip_prefix("record<")
        .and_then(|rest| rest.strip_suffix('>'))
    {
        let tables: Vec<surrealdb_types::Table> = targets
            .split('|')
            .map(|table| surrealdb_types::Table::from(table.trim()))
            .filter(|table| !table.to_string().is_empty())
            .collect();
        if !tables.is_empty() {
            return ParsedFieldKind {
                kind: Some(Kind::Record(tables)),
                partial: Vec::new(),
            };
        }
    }

    let kind = match normalized.as_str() {
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

#[cfg(test)]
mod tests {
    use surrealdb_types::Kind;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    use super::{extract_schema, FieldPath};

    #[test]
    fn schema_index_stores_define_statements_for_direct_table_and_field_lookup() {
        let parsed = parse_source(
            SourceId::new("schema:test"),
            "DEFINE TABLE user;\nDEFINE FIELD profile.name ON user TYPE string;\nDEFINE FIELD age ON user TYPE int;",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        assert_eq!(extraction.diagnostics, Vec::new());

        let user = extraction.schema.table("user").expect("user table exists");
        assert_eq!(user.name, "user");

        let profile_name = extraction
            .schema
            .field("user", &FieldPath::parse("profile.name"))
            .expect("profile.name field exists");
        assert_eq!(profile_name.kind, Some(Kind::String));

        let age = user
            .field(&FieldPath::parse("age"))
            .expect("age field exists");
        assert_eq!(age.kind, Some(Kind::Int));
    }

    #[test]
    fn table_field_helpers_enumerate_nested_field_prefixes() {
        let parsed = parse_source(
            SourceId::new("schema:nested"),
            "DEFINE TABLE user;\nDEFINE FIELD profile ON user TYPE object;\nDEFINE FIELD profile.name ON user TYPE string;\nDEFINE FIELD profile.age ON user TYPE int;\nDEFINE FIELD profiled.nickname ON user TYPE string;\nDEFINE FIELD email ON user TYPE string;",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        let user = extraction.schema.table("user").expect("user table exists");
        let nested: Vec<_> = user
            .fields_under(&FieldPath::parse("profile"))
            .map(|field| field.path.join("."))
            .collect();

        assert_eq!(nested, vec!["profile", "profile.age", "profile.name"]);
        assert!(user.field(&FieldPath::parse("profiled")).is_none());
        assert_eq!(FieldPath::parse("..profile.name.").dotted(), "profile.name");
    }

    #[test]
    fn schema_index_stores_params_functions_and_analyzers_for_direct_lookup() {
        let parsed = parse_source(
            SourceId::new("schema:param-function-analyzer"),
            "DEFINE PARAM $api_timeout VALUE 30;\nDEFINE FUNCTION fn::score($age: int) -> int { RETURN $age; };\nDEFINE ANALYZER ascii TOKENIZERS blank,class FILTERS lowercase,snowball(english);",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        assert_eq!(extraction.diagnostics, Vec::new());

        let param = extraction
            .schema
            .param("api_timeout")
            .expect("param is directly indexed without the $ sigil");
        assert_eq!(param.name, "api_timeout");
        assert_eq!(
            param.name_span.source(),
            &SourceId::new("schema:param-function-analyzer")
        );

        let function = extraction
            .schema
            .function("fn::score")
            .expect("function is directly indexed with namespace path");
        assert_eq!(function.name, "fn::score");
        assert_eq!(function.args, vec!["age"]);
        assert_eq!(function.return_kind, Some(Kind::Int));

        let analyzer = extraction
            .schema
            .analyzer("ascii")
            .expect("analyzer is directly indexed for full-text operator validation");
        assert_eq!(analyzer.name, "ascii");
        assert_eq!(analyzer.tokenizers, vec!["blank", "class"]);
        assert_eq!(analyzer.filters, vec!["lowercase", "snowball(english)"]);
    }

    #[test]
    fn define_table_overwrite_replaces_table_metadata_without_duplicate_diagnostic() {
        let parsed = parse_source(
            SourceId::new("schema:overwrite-table"),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD weight ON likes TYPE int;\nDEFINE TABLE OVERWRITE likes;",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        assert_eq!(extraction.diagnostics, Vec::new());

        let likes = extraction
            .schema
            .table("likes")
            .expect("overwritten table remains in schema");
        assert_eq!(likes.relation, None);
        assert_eq!(
            likes
                .field(&FieldPath::parse("weight"))
                .expect("field remains attached after table overwrite")
                .kind,
            Some(Kind::Int)
        );
    }

    #[test]
    fn duplicate_define_without_overwrite_emits_diagnostic() {
        let parsed = parse_source(
            SourceId::new("schema:duplicate-table"),
            "DEFINE TABLE person;\nDEFINE TABLE person;",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        assert_eq!(extraction.diagnostics.len(), 1);
        assert_eq!(
            extraction.diagnostics[0].message(),
            "duplicate table definition `person`"
        );
    }

    #[test]
    fn remove_table_and_field_mutate_downstream_schema_context() {
        let parsed = parse_source(
            SourceId::new("schema:remove"),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD age ON person TYPE int;\nREMOVE FIELD age ON person;\nREMOVE TABLE person;",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        assert_eq!(extraction.diagnostics, Vec::new());
        assert!(extraction.schema.table("person").is_none());
    }
}
