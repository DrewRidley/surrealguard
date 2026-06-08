use std::collections::BTreeMap;

use surrealguard_diagnostics::{Finding, FindingCode, Severity};
use surrealguard_syntax::parse::ParsedSource;
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use tree_sitter::Node;

use crate::analysis::{ParamInference, SelectModifierAnalysis, StatementAnalysis};
use crate::response_shape::{FieldShape, PartialReason, ResponseShape};
use crate::schema::SchemaIndex;
use crate::select_ir::{
    select_ir_from_statement, FieldPath, GraphDirection, SelectIr, SelectModifier, SelectProjection,
};

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

pub fn infer_select_response_shapes(
    parsed_sources: &[ParsedSource],
    schema: &SchemaIndex,
) -> Vec<(SourceSpan, ResponseShape)> {
    let mut shapes = Vec::new();

    for parsed in parsed_sources {
        if !parsed.syntax_diagnostics().is_empty() {
            continue;
        }
        collect_select_response_shapes(parsed.tree().root_node(), parsed, schema, &mut shapes);
    }

    shapes
}

fn collect_select_response_shapes(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    shapes: &mut Vec<(SourceSpan, ResponseShape)>,
) {
    if node.kind() == "SelectStatement" {
        let ir = select_ir_from_statement(node, parsed);
        shapes.push((
            node_span(node, parsed.source_id().clone()),
            response_shape_for_select(&ir, schema),
        ));
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_select_response_shapes(child, parsed, schema, shapes);
    }
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
            select_modifiers: select_modifier_analysis_for_node(node, parsed),
        });
        collect_params(node, parsed, params);
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_statement_analysis(child, parsed, statements, params);
    }
}

fn select_modifier_analysis_for_node(
    node: Node<'_>,
    parsed: &ParsedSource,
) -> Vec<SelectModifierAnalysis> {
    if node.kind() != "SelectStatement" {
        return Vec::new();
    }

    select_ir_from_statement(node, parsed)
        .modifiers
        .into_iter()
        .filter_map(|modifier| match modifier {
            SelectModifier::Where(span) => Some(row_preserving_modifier("where", span, None)),
            SelectModifier::Order(span) => Some(row_preserving_modifier("order", span, None)),
            SelectModifier::Limit { span, max_len } => {
                Some(row_preserving_modifier("limit", span, max_len))
            }
            SelectModifier::Start(span) => Some(row_preserving_modifier("start", span, None)),
            SelectModifier::Timeout(span) => Some(row_preserving_modifier("timeout", span, None)),
            SelectModifier::Parallel(span) => Some(row_preserving_modifier("parallel", span, None)),
            SelectModifier::Group(_) | SelectModifier::Split(_) | SelectModifier::Explain(_) => {
                None
            }
        })
        .collect()
}

fn row_preserving_modifier(
    kind: &str,
    span: SourceSpan,
    max_len: Option<u64>,
) -> SelectModifierAnalysis {
    SelectModifierAnalysis {
        kind: kind.to_string(),
        span,
        row_preserving: true,
        max_len,
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
    if node.kind() == "SelectStatement" {
        let ir = select_ir_from_statement(node, parsed);
        if let Some(source) = ir.source {
            if let Some(table_name) = source.table {
                if !schema.tables.contains_key(&table_name) {
                    diagnostics.push(Finding::new(
                        source.span,
                        FindingCode::schema(1003),
                        Severity::Error,
                        format!("unknown table `{table_name}` in SELECT statement"),
                    ));
                }
            }
        }
    } else {
        let references = match node.kind() {
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
    let ir = select_ir_from_statement(node, parsed);
    let Some(source) = ir.source else {
        return;
    };
    let Some(table_name) = source.table else {
        return;
    };
    let Some(table) = schema.tables.get(&table_name) else {
        return;
    };
    if table.fields.is_empty() {
        return;
    }

    for projection in ir.projections {
        let SelectProjection::Field { path, .. } = projection else {
            continue;
        };
        validate_field_path_on_table(path, table, diagnostics);
    }

    for path in ir.omit {
        validate_field_path_on_table(path, table, diagnostics);
    }

    for path in ir.fetch {
        validate_field_path_on_table(path, table, diagnostics);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "WhereClause" {
            for path in row_context_field_paths_from_clause(child, parsed) {
                validate_field_path_on_table(path, table, diagnostics);
            }
        }
    }
}

fn row_context_field_paths_from_clause(clause: Node<'_>, parsed: &ParsedSource) -> Vec<FieldPath> {
    let mut paths = Vec::new();
    let mut cursor = clause.walk();
    for child in clause.children(&mut cursor) {
        collect_row_context_field_paths(child, parsed, &mut paths);
    }
    paths
}

fn collect_row_context_field_paths(
    node: Node<'_>,
    parsed: &ParsedSource,
    paths: &mut Vec<FieldPath>,
) {
    if node.kind() == "VariableName" || node.kind() == "Keyword" {
        return;
    }

    if is_row_context_field_path_node(node) {
        paths.push(field_path_from_node(node, parsed));
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_row_context_field_paths(child, parsed, paths);
    }
}

fn is_row_context_field_path_node(node: Node<'_>) -> bool {
    matches!(node.kind(), "Ident" | "Path" | "Idiom")
}

fn field_path_from_node(node: Node<'_>, parsed: &ParsedSource) -> FieldPath {
    let text = node_text(node, parsed.text()).trim().to_string();
    FieldPath {
        segments: text.split('.').map(str::to_string).collect(),
        text,
        span: node_span(node, parsed.source_id().clone()),
    }
}

fn validate_field_path_on_table(
    path: FieldPath,
    table: &crate::schema::TableDef,
    diagnostics: &mut Vec<Finding>,
) {
    if !table.fields.contains_key(&path.text) {
        diagnostics.push(Finding::new(
            path.span,
            FindingCode::schema(1004),
            Severity::Error,
            format!("unknown field `{}` on table `{}`", path.text, table.name),
        ));
    }
}

fn response_shape_for_select(ir: &SelectIr, schema: &SchemaIndex) -> ResponseShape {
    if let Some(reason) = advanced_select_partial_reason(ir) {
        return ResponseShape::Unknown { reason };
    }

    let Some(source) = &ir.source else {
        return ResponseShape::Unknown {
            reason: PartialReason::Unresolved,
        };
    };
    if source.dynamic {
        return ResponseShape::Unknown {
            reason: PartialReason::DynamicExpression,
        };
    }
    let Some(table_name) = resolved_select_table_name(ir, schema) else {
        return ResponseShape::Unknown {
            reason: PartialReason::Unresolved,
        };
    };
    let Some(table) = schema.tables.get(table_name.as_str()) else {
        return ResponseShape::Unknown {
            reason: PartialReason::Unresolved,
        };
    };
    if table.fields.is_empty() {
        return ResponseShape::Unknown {
            reason: PartialReason::Unresolved,
        };
    }

    let row_shape = if ir
        .projections
        .iter()
        .any(|projection| matches!(projection, SelectProjection::Wildcard { .. }))
    {
        object_shape_for_all_fields(table)
    } else if let Some(value_shape) = value_projection_shape(ir, table) {
        value_shape
    } else {
        object_shape_for_projected_fields(ir, table)
    };
    let row_shape = apply_omit_to_shape(row_shape, &ir.omit);
    let row_shape = apply_fetch_materialization(row_shape, &ir.fetch);

    if ir.only {
        row_shape
    } else {
        ResponseShape::Array {
            element: Box::new(row_shape),
            max_len: literal_limit_max_len(ir),
        }
    }
}

fn resolved_select_table_name(ir: &SelectIr, schema: &SchemaIndex) -> Option<String> {
    let source_table = ir.source.as_ref()?.table.as_ref()?;
    if ir.graph_lookups.is_empty() {
        return Some(source_table.clone());
    }

    resolve_simple_graph_target_table(source_table, ir, schema)
}

fn resolve_simple_graph_target_table(
    source_table: &str,
    ir: &SelectIr,
    schema: &SchemaIndex,
) -> Option<String> {
    let [edge_lookup, target_lookup] = ir.graph_lookups.as_slice() else {
        return None;
    };
    let edge_table_name = edge_lookup.table.as_deref()?;
    let target_table_name = target_lookup.table.as_deref()?;
    let relation = schema.tables.get(edge_table_name)?.relation.as_ref()?;

    match edge_lookup.direction {
        GraphDirection::Out => {
            relation.in_tables.iter().any(|table| table == source_table)
                && relation
                    .out_tables
                    .iter()
                    .any(|table| table == target_table_name)
        }
        GraphDirection::In => {
            relation
                .out_tables
                .iter()
                .any(|table| table == source_table)
                && relation
                    .in_tables
                    .iter()
                    .any(|table| table == target_table_name)
        }
        GraphDirection::Both => {
            let source_is_in = relation.in_tables.iter().any(|table| table == source_table);
            let source_is_out = relation
                .out_tables
                .iter()
                .any(|table| table == source_table);
            let target_is_in = relation
                .in_tables
                .iter()
                .any(|table| table == target_table_name);
            let target_is_out = relation
                .out_tables
                .iter()
                .any(|table| table == target_table_name);
            (source_is_in && target_is_out) || (source_is_out && target_is_in)
        }
    }
    .then(|| target_table_name.to_string())
}

fn advanced_select_partial_reason(ir: &SelectIr) -> Option<PartialReason> {
    if ir.return_clause.is_some() {
        return Some(PartialReason::UnsupportedSyntax("RETURN".into()));
    }

    ir.modifiers.iter().find_map(|modifier| match modifier {
        SelectModifier::Group(_) => Some(PartialReason::UnsupportedSyntax("GROUP".into())),
        SelectModifier::Split(_) => Some(PartialReason::UnsupportedSyntax("SPLIT".into())),
        SelectModifier::Explain(_) => Some(PartialReason::UnsupportedSyntax("EXPLAIN".into())),
        SelectModifier::Where(_)
        | SelectModifier::Order(_)
        | SelectModifier::Limit { .. }
        | SelectModifier::Start(_)
        | SelectModifier::Timeout(_)
        | SelectModifier::Parallel(_) => None,
    })
}

fn literal_limit_max_len(ir: &SelectIr) -> Option<u64> {
    ir.modifiers.iter().find_map(|modifier| match modifier {
        SelectModifier::Limit { max_len, .. } => *max_len,
        _ => None,
    })
}

fn apply_omit_to_shape(shape: ResponseShape, omit: &[FieldPath]) -> ResponseShape {
    let ResponseShape::Object { mut fields, open } = shape else {
        return shape;
    };

    for omitted in omit {
        fields.remove(&omitted.text);
    }

    ResponseShape::Object { fields, open }
}

fn apply_fetch_materialization(shape: ResponseShape, fetch: &[FieldPath]) -> ResponseShape {
    let ResponseShape::Object { mut fields, open } = shape else {
        return shape;
    };

    for fetched in fetch {
        if let Some(field) = fields.get_mut(&fetched.text) {
            field.materialized_by_fetch = true;
        }
    }

    ResponseShape::Object { fields, open }
}

fn object_shape_for_all_fields(table: &crate::schema::TableDef) -> ResponseShape {
    let fields = table
        .fields
        .iter()
        .map(|(name, field)| {
            (
                name.clone(),
                field_shape_from_schema_field(field, field.name_span.clone()),
            )
        })
        .collect();

    ResponseShape::Object {
        fields,
        open: false,
    }
}

fn object_shape_for_projected_fields(
    ir: &SelectIr,
    table: &crate::schema::TableDef,
) -> ResponseShape {
    let mut fields = BTreeMap::new();

    for projection in &ir.projections {
        let SelectProjection::Field { path, alias, .. } = projection else {
            continue;
        };
        let Some(field) = table.fields.get(&path.text) else {
            fields.insert(
                alias.clone().unwrap_or_else(|| path.text.clone()),
                FieldShape {
                    shape: ResponseShape::Unknown {
                        reason: PartialReason::Unresolved,
                    },
                    kind: None,
                    span: path.span.clone(),
                    materialized_by_fetch: false,
                    partial: vec![PartialReason::Unresolved],
                },
            );
            continue;
        };
        fields.insert(
            alias.clone().unwrap_or_else(|| path.text.clone()),
            field_shape_from_schema_field(field, path.span.clone()),
        );
    }

    ResponseShape::Object {
        fields,
        open: false,
    }
}

fn value_projection_shape(ir: &SelectIr, table: &crate::schema::TableDef) -> Option<ResponseShape> {
    let [SelectProjection::Field {
        path, value: true, ..
    }] = ir.projections.as_slice()
    else {
        return None;
    };
    let field = table.fields.get(&path.text)?;
    Some(match field.kind.clone() {
        Some(kind) => ResponseShape::Value { kind },
        None => ResponseShape::Unknown {
            reason: field
                .partial
                .first()
                .cloned()
                .unwrap_or(PartialReason::Unresolved),
        },
    })
}

fn field_shape_from_schema_field(field: &crate::schema::FieldDef, span: SourceSpan) -> FieldShape {
    let shape = match field.kind.clone() {
        Some(kind) => ResponseShape::Value { kind },
        None => ResponseShape::Unknown {
            reason: field
                .partial
                .first()
                .cloned()
                .unwrap_or(PartialReason::Unresolved),
        },
    };

    FieldShape {
        shape,
        kind: field.kind.clone(),
        span,
        materialized_by_fetch: false,
        partial: field.partial.clone(),
    }
}

#[derive(Clone, Copy)]
struct TableReference<'tree> {
    name: &'tree str,
    node: Node<'tree>,
    statement: &'static str,
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
