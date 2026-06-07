use std::collections::BTreeMap;

use surrealguard_diagnostics::{Finding, FindingCode, Severity};
use surrealguard_syntax::parse::ParsedSource;
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use tree_sitter::Node;

use crate::analysis::{ParamInference, StatementAnalysis};
use crate::schema::SchemaIndex;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SemanticOutput {
    pub statements: Vec<StatementAnalysis>,
    pub inferred_params: Vec<ParamInference>,
}

pub fn analyze_parsed_source(parsed: &ParsedSource) -> SemanticOutput {
    if !parsed.syntax_diagnostics().is_empty() {
        return SemanticOutput::default();
    }

    let mut statements = Vec::new();
    let mut params = BTreeMap::new();
    collect_statement_analysis(
        parsed.tree().root_node(),
        parsed,
        &mut statements,
        &mut params,
    );

    SemanticOutput {
        statements,
        inferred_params: params.into_values().collect(),
    }
}

pub fn validate_table_references(
    parsed_sources: &[ParsedSource],
    schema: &SchemaIndex,
) -> Vec<Finding> {
    let mut diagnostics = Vec::new();

    for parsed in parsed_sources {
        if !parsed.syntax_diagnostics().is_empty() {
            continue;
        }

        collect_table_reference_diagnostics(
            parsed.tree().root_node(),
            parsed,
            schema,
            &mut diagnostics,
        );
    }

    diagnostics
}

pub fn validate_select_projection_fields(
    parsed_sources: &[ParsedSource],
    schema: &SchemaIndex,
) -> Vec<Finding> {
    let mut diagnostics = Vec::new();

    for parsed in parsed_sources {
        if !parsed.syntax_diagnostics().is_empty() {
            continue;
        }

        collect_select_projection_field_diagnostics(
            parsed.tree().root_node(),
            parsed,
            schema,
            &mut diagnostics,
        );
    }

    diagnostics
}

fn collect_statement_analysis(
    node: Node<'_>,
    parsed: &ParsedSource,
    statements: &mut Vec<StatementAnalysis>,
    params: &mut BTreeMap<String, ParamInference>,
) {
    if let Some(kind) = statement_kind(node, parsed.text()) {
        statements.push(StatementAnalysis {
            span: node_span(node, parsed.source_id().clone()),
            kind,
            response_shape: None,
        });
        collect_params(node, parsed, params);
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_statement_analysis(child, parsed, statements, params);
    }
}

fn collect_params(
    node: Node<'_>,
    parsed: &ParsedSource,
    params: &mut BTreeMap<String, ParamInference>,
) {
    if node.kind() == "VariableName" {
        let name = param_name(node_text(node, parsed.text()));
        params
            .entry(name.clone())
            .or_insert_with(|| ParamInference {
                name,
                kind: None,
                required: true,
                spans: Vec::new(),
            })
            .spans
            .push(node_span(node, parsed.source_id().clone()));
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_params(child, parsed, params);
    }
}

fn collect_table_reference_diagnostics(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    diagnostics: &mut Vec<Finding>,
) {
    let references = match node.kind() {
        "SelectStatement" => select_table_references(node, parsed.text()),
        "CreateStatement" => leading_table_references(node, parsed.text(), "CREATE"),
        "UpdateStatement" => leading_table_references(node, parsed.text(), "UPDATE"),
        "DeleteStatement" => leading_table_references(node, parsed.text(), "DELETE"),
        _ => Vec::new(),
    };

    for table_ref in references {
        if !schema.tables.contains_key(table_ref.name) {
            diagnostics.push(Finding::new(
                node_span(table_ref.node, parsed.source_id().clone()),
                FindingCode::schema(1003),
                Severity::Error,
                format!(
                    "unknown table `{}` in {} statement",
                    table_ref.name, table_ref.statement
                ),
            ));
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_table_reference_diagnostics(child, parsed, schema, diagnostics);
    }
}

fn collect_select_projection_field_diagnostics(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    diagnostics: &mut Vec<Finding>,
) {
    if node.kind() == "SelectStatement" {
        validate_select_projection_fields_for_statement(node, parsed, schema, diagnostics);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_select_projection_field_diagnostics(child, parsed, schema, diagnostics);
    }
}

fn validate_select_projection_fields_for_statement(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    diagnostics: &mut Vec<Finding>,
) {
    let Some(table_ref) = select_table_references(node, parsed.text())
        .into_iter()
        .next()
    else {
        return;
    };
    let Some(table) = schema.tables.get(table_ref.name) else {
        return;
    };
    if table.fields.is_empty() {
        return;
    }

    for field_ref in select_projection_fields(node, parsed.text()) {
        if !table.fields.contains_key(field_ref.name) {
            diagnostics.push(Finding::new(
                node_span(field_ref.node, parsed.source_id().clone()),
                FindingCode::schema(1004),
                Severity::Error,
                format!(
                    "unknown field `{}` on table `{}`",
                    field_ref.name, table.name
                ),
            ));
        }
    }
}

#[derive(Clone, Copy)]
struct TableReference<'tree> {
    name: &'tree str,
    node: Node<'tree>,
    statement: &'static str,
}

#[derive(Clone, Copy)]
struct FieldReference<'tree> {
    name: &'tree str,
    node: Node<'tree>,
}

fn select_table_references<'tree>(
    node: Node<'tree>,
    source: &'tree str,
) -> Vec<TableReference<'tree>> {
    let mut saw_from = false;
    let mut references = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        let text = node_text(child, source);
        if child.kind() == "Keyword" {
            let lower = text.to_ascii_lowercase();
            if lower == "from" {
                saw_from = true;
            }
            continue;
        }

        if !saw_from {
            continue;
        }

        if is_identifier_like(child) {
            references.push(TableReference {
                name: table_name_from_node_text(text),
                node: child,
                statement: "SELECT",
            });
        }
    }

    references
}

fn leading_table_references<'tree>(
    node: Node<'tree>,
    source: &'tree str,
    statement: &'static str,
) -> Vec<TableReference<'tree>> {
    let mut saw_statement_keyword = false;
    let mut references = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        let text = node_text(child, source);
        if child.kind() == "Keyword" {
            let lower = text.to_ascii_lowercase();
            if lower == statement.to_ascii_lowercase() {
                saw_statement_keyword = true;
            }
            continue;
        }

        if !saw_statement_keyword {
            continue;
        }

        if is_identifier_like(child) {
            references.push(TableReference {
                name: table_name_from_node_text(text),
                node: child,
                statement,
            });
            continue;
        }

        if is_data_or_modifier_clause(child) {
            break;
        }
    }

    references
}

fn select_projection_fields<'tree>(
    node: Node<'tree>,
    source: &'tree str,
) -> Vec<FieldReference<'tree>> {
    let mut cursor = node.walk();
    let Some(fields_node) = node
        .children(&mut cursor)
        .find(|child| child.kind() == "Fields")
    else {
        return Vec::new();
    };

    let mut references = Vec::new();
    let mut fields_cursor = fields_node.walk();
    for child in fields_node.children(&mut fields_cursor) {
        if child.kind() == "Any" {
            continue;
        }
        if child.kind() != "Predicate" {
            continue;
        }

        if let Some(field_node) = simple_projection_field_node(child) {
            references.push(FieldReference {
                name: node_text(field_node, source).trim(),
                node: field_node,
            });
        }
    }

    references
}

fn simple_projection_field_node(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    let mut children = node.children(&mut cursor).filter(|child| child.is_named());
    let child = children.next()?;
    if children.next().is_some() {
        return None;
    }

    if matches!(child.kind(), "Ident" | "Path") {
        Some(child)
    } else {
        None
    }
}

fn statement_kind(node: Node<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "DefineStatement" => define_statement_kind(node, source),
        "SelectStatement" => Some("select".into()),
        "CreateStatement" => Some("create".into()),
        "UpdateStatement" => Some("update".into()),
        "DeleteStatement" => Some("delete".into()),
        "InsertStatement" => Some("insert".into()),
        "RelateStatement" => Some("relate".into()),
        "LetStatement" => Some("let".into()),
        _ => None,
    }
}

fn define_statement_kind(node: Node<'_>, source: &str) -> Option<String> {
    let mut saw_define = false;
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        if child.kind() != "Keyword" {
            continue;
        }

        let keyword = node_text(child, source).to_ascii_lowercase();
        if !saw_define && keyword == "define" {
            saw_define = true;
            continue;
        }

        if saw_define {
            return Some(format!("define_{keyword}"));
        }
    }

    Some("define".into())
}

fn param_name(text: &str) -> String {
    text.trim_start_matches('$')
        .trim_matches('`')
        .trim_matches('⟨')
        .trim_matches('⟩')
        .to_string()
}

fn is_data_or_modifier_clause(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "ContentClause"
            | "SetClause"
            | "UnsetClause"
            | "WhereClause"
            | "ReturnClause"
            | "TimeoutClause"
            | "ParallelClause"
    )
}

fn is_identifier_like(node: Node<'_>) -> bool {
    matches!(node.kind(), "Ident" | "RecordId" | "Thing" | "Identifier")
}

fn table_name_from_node_text(text: &str) -> &str {
    text.split_once(':')
        .map(|(table, _)| table)
        .unwrap_or(text)
        .trim()
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
