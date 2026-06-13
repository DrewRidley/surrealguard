use std::collections::BTreeMap;

use surrealguard_diagnostics::{Finding, FindingCode, Severity};
use surrealguard_syntax::parse::ParsedSource;
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use surrealdb_types::Kind;
use tree_sitter::Node;

use crate::analysis::{ParamInference, SelectModifierAnalysis, StatementAnalysis};
use crate::expression::infer_expression_fact;
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamKindInference {
    pub source: SourceId,
    pub name: String,
    pub kind: Kind,
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

pub fn validate_select_graph_references(
    parsed_sources: &[ParsedSource],
    schema: &SchemaIndex,
) -> Vec<Finding> {
    let mut diagnostics = Vec::new();

    for parsed in parsed_sources {
        if !parsed.syntax_diagnostics().is_empty() {
            continue;
        }

        collect_select_graph_reference_diagnostics(
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

pub fn validate_mutation_fields(
    parsed_sources: &[ParsedSource],
    schema: &SchemaIndex,
) -> Vec<Finding> {
    let mut diagnostics = Vec::new();

    for parsed in parsed_sources {
        if !parsed.syntax_diagnostics().is_empty() {
            continue;
        }

        collect_mutation_field_diagnostics(
            parsed.tree().root_node(),
            parsed,
            schema,
            &mut diagnostics,
        );
    }

    diagnostics
}

pub fn infer_param_kinds(
    parsed_sources: &[ParsedSource],
    schema: &SchemaIndex,
) -> Vec<ParamKindInference> {
    let mut inferences = Vec::new();

    for parsed in parsed_sources {
        if !parsed.syntax_diagnostics().is_empty() {
            continue;
        }

        collect_select_param_kind_inferences(
            parsed.tree().root_node(),
            parsed,
            schema,
            &mut inferences,
        );
        collect_mutation_param_kind_inferences(
            parsed.tree().root_node(),
            parsed,
            schema,
            &mut inferences,
        );
    }

    inferences
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

pub fn infer_non_select_response_shapes(
    parsed_sources: &[ParsedSource],
    schema: &SchemaIndex,
) -> Vec<(SourceSpan, ResponseShape)> {
    let mut shapes = Vec::new();

    for parsed in parsed_sources {
        if !parsed.syntax_diagnostics().is_empty() {
            continue;
        }
        collect_non_select_response_shapes(parsed.tree().root_node(), parsed, schema, &mut shapes);
    }

    shapes
}

fn collect_non_select_response_shapes(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    shapes: &mut Vec<(SourceSpan, ResponseShape)>,
) {
    if matches!(
        node.kind(),
        "CreateStatement"
            | "InsertStatement"
            | "UpdateStatement"
            | "UpsertStatement"
            | "DeleteStatement"
            | "RelateStatement"
    ) {
        shapes.push((
            node_span(node, parsed.source_id().clone()),
            response_shape_for_mutation(node, parsed, schema),
        ));
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_non_select_response_shapes(child, parsed, schema, shapes);
    }
}

fn response_shape_for_mutation(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
) -> ResponseShape {
    match mutation_return_mode(node, parsed) {
        MutationReturnMode::None => ResponseShape::Array {
            element: Box::new(ResponseShape::Unknown {
                reason: PartialReason::UnsupportedSyntax("RETURN NONE".into()),
            }),
            max_len: Some(0),
        },
        MutationReturnMode::Diff => ResponseShape::Unknown {
            reason: PartialReason::UnsupportedSyntax("RETURN DIFF".into()),
        },
        MutationReturnMode::Fields => ResponseShape::Unknown {
            reason: PartialReason::UnsupportedSyntax("RETURN fields".into()),
        },
        MutationReturnMode::Rows => {
            let Some(table_name) = mutation_table_name(node, parsed) else {
                return ResponseShape::Unknown {
                    reason: PartialReason::Unresolved,
                };
            };
            let Some(table) = schema.tables.get(&table_name) else {
                return ResponseShape::Unknown {
                    reason: PartialReason::Unresolved,
                };
            };
            if table.fields.is_empty() {
                return ResponseShape::Unknown {
                    reason: PartialReason::Unresolved,
                };
            }
            ResponseShape::Array {
                element: Box::new(object_shape_for_all_fields(table)),
                max_len: None,
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MutationReturnMode {
    Rows,
    None,
    Diff,
    Fields,
}

fn mutation_return_mode(node: Node<'_>, parsed: &ParsedSource) -> MutationReturnMode {
    let Some(return_clause) = find_descendant_kind(node, "ReturnClause") else {
        return MutationReturnMode::Rows;
    };
    let text = node_text(return_clause, parsed.text()).to_ascii_uppercase();
    if text.contains("NONE") {
        MutationReturnMode::None
    } else if text.contains("DIFF") {
        MutationReturnMode::Diff
    } else if text.contains("BEFORE") || text.contains("AFTER") {
        MutationReturnMode::Rows
    } else {
        MutationReturnMode::Fields
    }
}

fn find_descendant_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    if node.kind() == kind {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = find_descendant_kind(child, kind) {
            return Some(found);
        }
    }
    None
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
            "UpsertStatement" => leading_table_references(node, parsed.text(), "UPSERT"),
            "InsertStatement" => {
                table_references_after_keyword(node, parsed.text(), "INTO", "INSERT")
            }
            "LiveSelectStatement" => {
                table_references_after_keyword(node, parsed.text(), "FROM", "LIVE SELECT")
            }
            "AlterStatement" => {
                table_references_after_keyword(node, parsed.text(), "TABLE", "ALTER")
            }
            "RemoveStatement" => {
                table_references_after_keyword(node, parsed.text(), "TABLE", "REMOVE")
            }
            "RebuildStatement" => {
                table_references_after_keyword(node, parsed.text(), "TABLE", "REBUILD")
            }
            "ShowStatement" => table_references_after_keyword(node, parsed.text(), "TABLE", "SHOW"),
            "InfoForStatement" => {
                let mut references =
                    table_references_after_keyword(node, parsed.text(), "TABLE", "INFO FOR");
                references.extend(table_references_after_keyword(
                    node,
                    parsed.text(),
                    "TB",
                    "INFO FOR",
                ));
                references
            }
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

fn collect_select_graph_reference_diagnostics(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    diagnostics: &mut Vec<Finding>,
) {
    match node.kind() {
        "SelectStatement" => {
            validate_graph_references_for_select_statement(node, parsed, schema, diagnostics)
        }
        "RelateStatement" => {
            validate_graph_references_for_relate_statement(node, parsed, schema, diagnostics)
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_select_graph_reference_diagnostics(child, parsed, schema, diagnostics);
    }
}

fn validate_graph_references_for_select_statement(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    diagnostics: &mut Vec<Finding>,
) {
    let ir = select_ir_from_statement(node, parsed);
    if ir.graph_lookups.is_empty() {
        return;
    }

    let [edge_lookup, target_lookup] = ir.graph_lookups.as_slice() else {
        return;
    };
    let Some(source_table) = ir
        .source
        .as_ref()
        .and_then(|source| source.table.as_deref())
    else {
        return;
    };

    let Some(edge_table) = edge_lookup.table.as_deref() else {
        return;
    };
    let edge_relation = schema
        .tables
        .get(edge_table)
        .and_then(|table| table.relation.as_ref());
    if edge_relation.is_none() {
        diagnostics.push(Finding::new(
            edge_lookup.span.clone(),
            FindingCode::graph(3001),
            Severity::Error,
            format!("unknown graph edge table `{edge_table}`"),
        ));
    }

    let Some(target_table) = target_lookup.table.as_deref() else {
        return;
    };
    let target_exists = schema.tables.contains_key(target_table);
    if !target_exists {
        diagnostics.push(Finding::new(
            target_lookup.span.clone(),
            FindingCode::graph(3002),
            Severity::Error,
            format!("unknown graph target table `{target_table}`"),
        ));
    }

    if edge_relation.is_some()
        && target_exists
        && resolve_simple_graph_target_table(source_table, &ir, schema).is_none()
    {
        diagnostics.push(Finding::new(
            node_span(node, parsed.source_id().clone()),
            FindingCode::graph(3003),
            Severity::Error,
            format!(
                "graph traversal `{source_table}->{edge_table}->{target_table}` does not match relation `{edge_table}` endpoints"
            ),
        ));
    }
}

fn validate_graph_references_for_relate_statement(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    diagnostics: &mut Vec<Finding>,
) {
    let Some(relate) = relate_graph_reference(node, parsed) else {
        return;
    };

    let source_exists = schema.tables.contains_key(&relate.source_table);
    if !source_exists {
        diagnostics.push(Finding::new(
            relate.source_span.clone(),
            FindingCode::graph(3002),
            Severity::Error,
            format!("unknown RELATE source table `{}`", relate.source_table),
        ));
    }

    let edge_relation = schema
        .tables
        .get(&relate.edge_table)
        .and_then(|table| table.relation.as_ref());
    if edge_relation.is_none() {
        diagnostics.push(Finding::new(
            relate.edge_span.clone(),
            FindingCode::graph(3001),
            Severity::Error,
            format!("unknown RELATE edge table `{}`", relate.edge_table),
        ));
    }

    let target_exists = schema.tables.contains_key(&relate.target_table);
    if !target_exists {
        diagnostics.push(Finding::new(
            relate.target_span.clone(),
            FindingCode::graph(3002),
            Severity::Error,
            format!("unknown RELATE target table `{}`", relate.target_table),
        ));
    }

    if let Some(relation) = edge_relation {
        let endpoints_match = relation
            .in_tables
            .iter()
            .any(|table| table == &relate.source_table)
            && relation
                .out_tables
                .iter()
                .any(|table| table == &relate.target_table);
        if source_exists && target_exists && !endpoints_match {
            diagnostics.push(Finding::new(
                node_span(node, parsed.source_id().clone()),
                FindingCode::graph(3003),
                Severity::Error,
                format!(
                    "RELATE traversal `{}->{}->{}` does not match relation `{}` endpoints",
                    relate.source_table, relate.edge_table, relate.target_table, relate.edge_table
                ),
            ));
        }
    }
}

struct RelateGraphReference {
    source_table: String,
    source_span: SourceSpan,
    edge_table: String,
    edge_span: SourceSpan,
    target_table: String,
    target_span: SourceSpan,
}

fn relate_graph_reference(node: Node<'_>, parsed: &ParsedSource) -> Option<RelateGraphReference> {
    let mut cursor = node.walk();
    let mut children = node.children(&mut cursor);
    let source = children.find(|child| child.kind() == "RecordId")?;
    let edge = children.find(|child| is_identifier_like(*child))?;
    let target = children.find(|child| child.kind() == "RecordId")?;

    Some(RelateGraphReference {
        source_table: table_name_from_node_text(node_text(source, parsed.text())).to_string(),
        source_span: node_span(source, parsed.source_id().clone()),
        edge_table: table_name_from_node_text(node_text(edge, parsed.text())).to_string(),
        edge_span: node_span(edge, parsed.source_id().clone()),
        target_table: table_name_from_node_text(node_text(target, parsed.text())).to_string(),
        target_span: node_span(target, parsed.source_id().clone()),
    })
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
    validate_graph_local_where_fields_for_select_statement(node, parsed, schema, diagnostics);

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

fn collect_mutation_field_diagnostics(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    diagnostics: &mut Vec<Finding>,
) {
    validate_mutation_fields_for_statement(node, parsed, schema, diagnostics);

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_mutation_field_diagnostics(child, parsed, schema, diagnostics);
    }
}

fn validate_mutation_fields_for_statement(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    diagnostics: &mut Vec<Finding>,
) {
    let table_name = mutation_table_name(node, parsed);
    let Some(table_name) = table_name else {
        return;
    };
    let Some(table) = schema.tables.get(&table_name) else {
        return;
    };
    if table.fields.is_empty() {
        return;
    }

    validate_assignment_fields_on_table(node, parsed, table, diagnostics);
    validate_object_fields_on_table(node, parsed, table, diagnostics);
    validate_insert_column_fields_on_table(node, parsed, table, diagnostics);
    validate_where_descendants_on_table(node, parsed, table, diagnostics);
    validate_mutation_value_assignability(node, parsed, table, diagnostics);
}

fn mutation_table_name(node: Node<'_>, parsed: &ParsedSource) -> Option<String> {
    match node.kind() {
        "CreateStatement" => leading_table_references(node, parsed.text(), "CREATE")
            .first()
            .map(|reference| reference.name.to_string()),
        "UpdateStatement" => leading_table_references(node, parsed.text(), "UPDATE")
            .first()
            .map(|reference| reference.name.to_string()),
        "DeleteStatement" => leading_table_references(node, parsed.text(), "DELETE")
            .first()
            .map(|reference| reference.name.to_string()),
        "UpsertStatement" => leading_table_references(node, parsed.text(), "UPSERT")
            .first()
            .map(|reference| reference.name.to_string()),
        "InsertStatement" => table_references_after_keyword(node, parsed.text(), "INTO", "INSERT")
            .first()
            .map(|reference| reference.name.to_string()),
        "RelateStatement" => relate_edge_table_name(node, parsed),
        _ => None,
    }
}

fn validate_assignment_fields_on_table(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    diagnostics: &mut Vec<Finding>,
) {
    if node.kind() == "FieldAssignment" {
        if let Some(path) = assignment_field_path(node, parsed) {
            validate_field_path_on_table(path, table, diagnostics);
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        validate_assignment_fields_on_table(child, parsed, table, diagnostics);
    }
}

fn assignment_field_path(assignment: Node<'_>, parsed: &ParsedSource) -> Option<FieldPath> {
    let mut cursor = assignment.walk();
    let path = assignment
        .children(&mut cursor)
        .find(|child| is_row_context_field_path_node(*child))
        .map(|child| field_path_from_node(child, parsed));
    path
}

fn validate_object_fields_on_table(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    diagnostics: &mut Vec<Finding>,
) {
    match node.kind() {
        "ContentClause" | "MergeClause" | "ReplaceClause" | "BulkInsert" => {
            validate_object_descendants_on_table(node, parsed, table, diagnostics);
            return;
        }
        "InsertStatement" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "Object" || child.kind() == "BulkInsert" {
                    validate_object_descendants_on_table(child, parsed, table, diagnostics);
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        validate_object_fields_on_table(child, parsed, table, diagnostics);
    }
}

fn validate_object_descendants_on_table(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    diagnostics: &mut Vec<Finding>,
) {
    if node.kind() == "Object" {
        validate_object_properties_on_table(node, parsed, table, Vec::new(), diagnostics);
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        validate_object_descendants_on_table(child, parsed, table, diagnostics);
    }
}

fn validate_object_properties_on_table(
    object: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    prefix: Vec<String>,
    diagnostics: &mut Vec<Finding>,
) {
    if object.kind() == "ObjectProperty" {
        let Some(key) = object_property_key(object, parsed) else {
            return;
        };
        let segments: Vec<_> = prefix
            .iter()
            .cloned()
            .chain(std::iter::once(key.name.clone()))
            .collect();
        let path = FieldPath {
            text: segments.join("."),
            segments: segments.clone(),
            span: key.span,
        };
        validate_field_path_on_table(path, table, diagnostics);

        if let Some(value_object) = object_property_value_object(object) {
            validate_object_properties_on_table(value_object, parsed, table, segments, diagnostics);
        }
        return;
    }

    let mut cursor = object.walk();
    for child in object.children(&mut cursor) {
        if child.kind() != "Object" {
            validate_object_properties_on_table(child, parsed, table, prefix.clone(), diagnostics);
        }
    }
}

struct ObjectKey {
    name: String,
    span: SourceSpan,
}

fn object_property_key(property: Node<'_>, parsed: &ParsedSource) -> Option<ObjectKey> {
    let mut cursor = property.walk();
    let key = property
        .children(&mut cursor)
        .find(|child| child.kind() == "ObjectKey")?;
    Some(ObjectKey {
        name: normalize_object_key(node_text(key, parsed.text())),
        span: node_span(key, parsed.source_id().clone()),
    })
}

fn object_property_value_object(property: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = property.walk();
    let object = property
        .children(&mut cursor)
        .find(|child| child.kind() == "Object");
    object
}

fn validate_insert_column_fields_on_table(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    diagnostics: &mut Vec<Finding>,
) {
    if node.kind() != "InsertStatement" {
        return;
    }
    let Some(table_reference) =
        table_references_after_keyword(node, parsed.text(), "INTO", "INSERT")
            .into_iter()
            .next()
    else {
        return;
    };

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "Ident" && child.start_byte() > table_reference.node.end_byte() {
            validate_field_path_on_table(field_path_from_node(child, parsed), table, diagnostics);
        }
    }
}

fn validate_mutation_value_assignability(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    diagnostics: &mut Vec<Finding>,
) {
    validate_assignment_value_assignability(node, parsed, table, diagnostics);
    validate_object_value_assignability(node, parsed, table, diagnostics);
    validate_insert_tuple_value_assignability(node, parsed, table, diagnostics);
}

fn validate_assignment_value_assignability(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    diagnostics: &mut Vec<Finding>,
) {
    if node.kind() == "FieldAssignment" {
        if let (Some(path), Some(value)) = (
            assignment_field_path(node, parsed),
            assignment_value_node(node),
        ) {
            validate_value_kind_for_field(path, value, parsed, table, diagnostics);
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        validate_assignment_value_assignability(child, parsed, table, diagnostics);
    }
}

fn assignment_value_node(assignment: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = assignment.walk();
    assignment
        .children(&mut cursor)
        .filter(|child| child.is_named() && !is_row_context_field_path_node(*child))
        .last()
}

fn validate_object_value_assignability(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    diagnostics: &mut Vec<Finding>,
) {
    match node.kind() {
        "ContentClause" | "MergeClause" | "ReplaceClause" | "BulkInsert" => {
            validate_object_value_descendants(node, parsed, table, diagnostics);
            return;
        }
        "InsertStatement" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "Object" || child.kind() == "BulkInsert" {
                    validate_object_value_descendants(child, parsed, table, diagnostics);
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        validate_object_value_assignability(child, parsed, table, diagnostics);
    }
}

fn validate_object_value_descendants(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    diagnostics: &mut Vec<Finding>,
) {
    if node.kind() == "Object" {
        validate_object_property_values_on_table(node, parsed, table, Vec::new(), diagnostics);
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        validate_object_value_descendants(child, parsed, table, diagnostics);
    }
}

fn validate_object_property_values_on_table(
    object: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    prefix: Vec<String>,
    diagnostics: &mut Vec<Finding>,
) {
    if object.kind() == "ObjectProperty" {
        let Some(key) = object_property_key(object, parsed) else {
            return;
        };
        let segments: Vec<_> = prefix
            .iter()
            .cloned()
            .chain(std::iter::once(key.name.clone()))
            .collect();
        let path = FieldPath {
            text: segments.join("."),
            segments: segments.clone(),
            span: key.span,
        };

        if let Some(value_object) = object_property_value_object(object) {
            validate_object_property_values_on_table(
                value_object,
                parsed,
                table,
                segments,
                diagnostics,
            );
        } else if let Some(value) = object_property_value_node(object) {
            validate_value_kind_for_field(path, value, parsed, table, diagnostics);
        }
        return;
    }

    let mut cursor = object.walk();
    for child in object.children(&mut cursor) {
        if child.kind() != "Object" {
            validate_object_property_values_on_table(
                child,
                parsed,
                table,
                prefix.clone(),
                diagnostics,
            );
        }
    }
}

fn object_property_value_node(property: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = property.walk();
    property
        .children(&mut cursor)
        .filter(|child| child.is_named() && child.kind() != "ObjectKey")
        .last()
}

fn validate_insert_tuple_value_assignability(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    diagnostics: &mut Vec<Finding>,
) {
    if node.kind() != "InsertStatement" {
        return;
    }
    let Some(table_reference) =
        table_references_after_keyword(node, parsed.text(), "INTO", "INSERT")
            .into_iter()
            .next()
    else {
        return;
    };

    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();
    let Some(values_keyword_index) = children.iter().position(|child| {
        child.kind() == "Keyword" && node_text(*child, parsed.text()).eq_ignore_ascii_case("VALUES")
    }) else {
        return;
    };

    let columns: Vec<_> = children
        .iter()
        .take(values_keyword_index)
        .filter(|child| {
            child.kind() == "Ident" && child.start_byte() > table_reference.node.end_byte()
        })
        .map(|child| field_path_from_node(*child, parsed))
        .collect();
    let values: Vec<_> = children
        .iter()
        .skip(values_keyword_index + 1)
        .filter(|child| child.is_named())
        .copied()
        .collect();

    if columns.is_empty() {
        return;
    }

    if values.len() % columns.len() != 0 {
        diagnostics.push(Finding::new(
            node_span(node, parsed.source_id().clone()),
            FindingCode::type_error(2002),
            Severity::Error,
            format!(
                "INSERT tuple has {} {} for {} {}",
                values.len(),
                pluralize(values.len(), "value", "values"),
                columns.len(),
                pluralize(columns.len(), "field", "fields")
            ),
        ));
    }

    for (index, value) in values.into_iter().enumerate() {
        if let Some(path) = columns.get(index % columns.len()).cloned() {
            validate_value_kind_for_field(path, value, parsed, table, diagnostics);
        }
    }
}

fn pluralize(count: usize, singular: &'static str, plural: &'static str) -> &'static str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

fn validate_value_kind_for_field(
    path: FieldPath,
    value: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    diagnostics: &mut Vec<Finding>,
) {
    let Some(field) = exact_field_def_for_path(table, &path) else {
        return;
    };
    let Some(expected) = field.kind.clone() else {
        return;
    };
    let fact = infer_expression_fact(value, parsed, Some(table));
    let Some(actual) = fact.kind else {
        return;
    };
    if kind_is_assignable_to(&actual, &expected) {
        return;
    }

    diagnostics.push(Finding::new(
        fact.span,
        FindingCode::type_error(2001),
        Severity::Error,
        format!(
            "value assigned to `{}` has type `{}`, expected `{}`",
            path.text,
            kind_name(&actual),
            kind_name(&expected)
        ),
    ));
}

fn kind_is_assignable_to(actual: &Kind, expected: &Kind) -> bool {
    if matches!(expected, Kind::Any) || actual == expected {
        return true;
    }
    matches!(
        (actual, expected),
        (
            Kind::Int | Kind::Float | Kind::Decimal | Kind::Number,
            Kind::Number
        ) | (Kind::Int, Kind::Float)
            | (Kind::Int, Kind::Decimal)
    )
}

fn kind_name(kind: &Kind) -> &'static str {
    match kind {
        Kind::Any => "any",
        Kind::None => "none",
        Kind::Null => "null",
        Kind::Bool => "bool",
        Kind::Bytes => "bytes",
        Kind::Datetime => "datetime",
        Kind::Decimal => "decimal",
        Kind::Duration => "duration",
        Kind::Float => "float",
        Kind::Int => "int",
        Kind::Number => "number",
        Kind::Object => "object",
        Kind::String => "string",
        Kind::Uuid => "uuid",
        Kind::Regex => "regex",
        Kind::Table(_) => "table",
        Kind::Record(_) => "record",
        Kind::Geometry(_) => "geometry",
        Kind::Either(_) => "either",
        Kind::Set(_, _) => "set",
        Kind::Array(_, _) => "array",
        Kind::Function(_, _) => "function",
        Kind::Range => "range",
        Kind::Literal(_) => "literal",
        Kind::File(_) => "file",
    }
}

fn relate_edge_table_name(node: Node<'_>, parsed: &ParsedSource) -> Option<String> {
    let mut saw_first_lookup = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "LookupRight" | "LookupLeft") {
            saw_first_lookup = true;
            continue;
        }
        if saw_first_lookup && is_identifier_like(child) {
            return Some(table_name_from_node_text(node_text(child, parsed.text())).to_string());
        }
    }
    None
}

fn normalize_object_key(text: &str) -> String {
    text.trim()
        .trim_matches('`')
        .trim_matches('⟨')
        .trim_matches('⟩')
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

fn validate_graph_local_where_fields_for_select_statement(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    diagnostics: &mut Vec<Finding>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        validate_graph_local_where_fields_in_node(child, parsed, schema, diagnostics);
    }
}

fn validate_graph_local_where_fields_in_node(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    diagnostics: &mut Vec<Finding>,
) {
    if node.kind() == "Path" {
        validate_graph_local_where_fields_in_path(node, parsed, schema, diagnostics);
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        validate_graph_local_where_fields_in_node(child, parsed, schema, diagnostics);
    }
}

fn validate_graph_local_where_fields_in_path(
    path: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    diagnostics: &mut Vec<Finding>,
) {
    let mut cursor = path.walk();
    let children: Vec<_> = path.children(&mut cursor).collect();

    for (index, child) in children.iter().copied().enumerate() {
        if child.kind() != "Lookup" {
            continue;
        }
        let Some(edge_table_name) = graph_lookup_table_name_from_node(child, parsed) else {
            continue;
        };
        let Some(edge_table) = schema.tables.get(&edge_table_name) else {
            continue;
        };
        if edge_table.fields.is_empty() {
            continue;
        }

        validate_where_descendants_on_table(child, parsed, edge_table, diagnostics);
        if let Some(next) = children.get(index + 1).copied() {
            if next.kind() == "Filter" {
                validate_where_descendants_on_table(next, parsed, edge_table, diagnostics);
            }
        }
    }
}

fn validate_where_descendants_on_table(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    diagnostics: &mut Vec<Finding>,
) {
    if node.kind() == "WhereClause" {
        for path in row_context_field_paths_from_clause(node, parsed) {
            validate_field_path_on_table(path, table, diagnostics);
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        validate_where_descendants_on_table(child, parsed, table, diagnostics);
    }
}

fn graph_lookup_table_name_from_node(node: Node<'_>, parsed: &ParsedSource) -> Option<String> {
    if is_identifier_like(node) {
        return Some(node_text(node, parsed.text()).trim().to_string());
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(name) = graph_lookup_table_name_from_node(child, parsed) {
            return Some(name);
        }
    }
    None
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

fn collect_select_param_kind_inferences(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    inferences: &mut Vec<ParamKindInference>,
) {
    if node.kind() == "SelectStatement" {
        infer_param_kinds_for_select_statement(node, parsed, schema, inferences);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_select_param_kind_inferences(child, parsed, schema, inferences);
    }
}

fn infer_param_kinds_for_select_statement(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    inferences: &mut Vec<ParamKindInference>,
) {
    let ir = select_ir_from_statement(node, parsed);
    collect_graph_local_param_kind_inferences_for_select_statement(
        node, parsed, schema, inferences,
    );

    let Some(table_name) = resolved_select_table_name(&ir, schema) else {
        return;
    };
    let Some(table) = schema.tables.get(table_name.as_str()) else {
        return;
    };

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "WhereClause" {
            collect_param_kind_inferences_from_expression(child, parsed, table, inferences);
        }
    }
}

fn collect_mutation_param_kind_inferences(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    inferences: &mut Vec<ParamKindInference>,
) {
    if matches!(
        node.kind(),
        "UpdateStatement" | "UpsertStatement" | "DeleteStatement"
    ) {
        infer_param_kinds_for_mutation_statement(node, parsed, schema, inferences);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_mutation_param_kind_inferences(child, parsed, schema, inferences);
    }
}

fn infer_param_kinds_for_mutation_statement(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    inferences: &mut Vec<ParamKindInference>,
) {
    let Some(table_name) = mutation_table_name(node, parsed) else {
        return;
    };
    let Some(table) = schema.tables.get(&table_name) else {
        return;
    };

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "WhereClause" {
            collect_param_kind_inferences_from_expression(child, parsed, table, inferences);
        }
    }
}

fn collect_graph_local_param_kind_inferences_for_select_statement(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    inferences: &mut Vec<ParamKindInference>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_graph_local_param_kind_inferences_in_node(child, parsed, schema, inferences);
    }
}

fn collect_graph_local_param_kind_inferences_in_node(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    inferences: &mut Vec<ParamKindInference>,
) {
    if node.kind() == "Path" {
        collect_graph_local_param_kind_inferences_in_path(node, parsed, schema, inferences);
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_graph_local_param_kind_inferences_in_node(child, parsed, schema, inferences);
    }
}

fn collect_graph_local_param_kind_inferences_in_path(
    path: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    inferences: &mut Vec<ParamKindInference>,
) {
    let mut cursor = path.walk();
    let children: Vec<_> = path.children(&mut cursor).collect();

    for (index, child) in children.iter().copied().enumerate() {
        if child.kind() != "Lookup" {
            continue;
        }
        let Some(edge_table_name) = graph_lookup_table_name_from_node(child, parsed) else {
            continue;
        };
        let Some(edge_table) = schema.tables.get(&edge_table_name) else {
            continue;
        };

        collect_param_kind_inferences_from_where_descendants(child, parsed, edge_table, inferences);
        if let Some(next) = children.get(index + 1).copied() {
            if next.kind() == "Filter" {
                collect_param_kind_inferences_from_where_descendants(
                    next, parsed, edge_table, inferences,
                );
            }
        }
    }
}

fn collect_param_kind_inferences_from_where_descendants(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    inferences: &mut Vec<ParamKindInference>,
) {
    if node.kind() == "WhereClause" {
        collect_param_kind_inferences_from_expression(node, parsed, table, inferences);
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_param_kind_inferences_from_where_descendants(child, parsed, table, inferences);
    }
}

fn collect_param_kind_inferences_from_expression(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    inferences: &mut Vec<ParamKindInference>,
) {
    if node.kind() == "BinaryExpression" {
        if let Some((field, param_name)) = direct_field_param_comparison(node, parsed) {
            if let Some(field_def) = exact_field_def_for_path(table, &field) {
                if let Some(kind) = field_def.kind.clone() {
                    inferences.push(ParamKindInference {
                        source: parsed.source_id().clone(),
                        name: param_name,
                        kind,
                    });
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_param_kind_inferences_from_expression(child, parsed, table, inferences);
    }
}

fn direct_field_param_comparison(
    node: Node<'_>,
    parsed: &ParsedSource,
) -> Option<(FieldPath, String)> {
    let mut cursor = node.walk();
    let children: Vec<_> = node
        .children(&mut cursor)
        .filter(|child| child.is_named())
        .collect();

    for (operator_index, child) in children.iter().copied().enumerate() {
        if child.kind() != "Operator"
            || !is_kind_inference_operator(node_text(child, parsed.text()).trim())
        {
            continue;
        }

        let left =
            nearest_inference_operand(&children[..operator_index], parsed, OperandSide::Left)?;
        let right =
            nearest_inference_operand(&children[operator_index + 1..], parsed, OperandSide::Right)?;
        match (left, right) {
            (InferenceOperand::Field(field), InferenceOperand::Param(param))
            | (InferenceOperand::Param(param), InferenceOperand::Field(field)) => {
                return Some((field, param));
            }
            _ => {}
        }
    }

    None
}

#[derive(Clone, Debug)]
enum InferenceOperand {
    Field(FieldPath),
    Param(String),
}

#[derive(Clone, Copy, Debug)]
enum OperandSide {
    Left,
    Right,
}

fn nearest_inference_operand(
    nodes: &[Node<'_>],
    parsed: &ParsedSource,
    side: OperandSide,
) -> Option<InferenceOperand> {
    let ordered_nodes: Box<dyn Iterator<Item = Node<'_>> + '_> = match side {
        OperandSide::Left => Box::new(nodes.iter().rev().copied()),
        OperandSide::Right => Box::new(nodes.iter().copied()),
    };

    for node in ordered_nodes {
        if let Some(operand) = nearest_inference_operand_in_node(node, parsed, side) {
            return Some(operand);
        }
    }
    None
}

fn nearest_inference_operand_in_node(
    node: Node<'_>,
    parsed: &ParsedSource,
    side: OperandSide,
) -> Option<InferenceOperand> {
    match node.kind() {
        "VariableName" => {
            return Some(InferenceOperand::Param(param_name(node_text(
                node,
                parsed.text(),
            ))));
        }
        "Keyword" | "Operator" => return None,
        _ if is_row_context_field_path_node(node) => {
            return Some(InferenceOperand::Field(field_path_from_node(node, parsed)));
        }
        _ => {}
    }

    let mut cursor = node.walk();
    let children: Vec<_> = node
        .children(&mut cursor)
        .filter(|child| child.is_named())
        .collect();
    nearest_inference_operand(&children, parsed, side)
}

fn is_kind_inference_operator(operator: &str) -> bool {
    matches!(operator, "=" | "!=" | "<" | "<=" | ">" | ">=")
}

fn validate_field_path_on_table(
    path: FieldPath,
    table: &crate::schema::TableDef,
    diagnostics: &mut Vec<Finding>,
) {
    if !field_path_exists_on_table(table, &path) {
        diagnostics.push(Finding::new(
            path.span,
            FindingCode::schema(1004),
            Severity::Error,
            format!("unknown field `{}` on table `{}`", path.text, table.name),
        ));
    }
}

fn exact_field_def_for_path<'a>(
    table: &'a crate::schema::TableDef,
    path: &FieldPath,
) -> Option<&'a crate::schema::FieldDef> {
    table.fields.get(&path.segments.join("."))
}

fn field_path_exists_on_table(table: &crate::schema::TableDef, path: &FieldPath) -> bool {
    exact_field_def_for_path(table, path).is_some()
        || table.fields.values().any(|field| {
            field.path.len() > path.segments.len() && field.path.starts_with(&path.segments)
        })
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
        remove_field_path_from_object_fields(&mut fields, &omitted.segments);
    }

    ResponseShape::Object { fields, open }
}

fn remove_field_path_from_object_fields(
    fields: &mut BTreeMap<String, FieldShape>,
    segments: &[String],
) {
    let Some((first, rest)) = segments.split_first() else {
        return;
    };

    if rest.is_empty() {
        fields.remove(first);
        return;
    }

    let Some(field) = fields.get_mut(first) else {
        return;
    };
    let ResponseShape::Object {
        fields: child_fields,
        ..
    } = &mut field.shape
    else {
        return;
    };
    remove_field_path_from_object_fields(child_fields, rest);
}

fn apply_fetch_materialization(shape: ResponseShape, fetch: &[FieldPath]) -> ResponseShape {
    let ResponseShape::Object { mut fields, open } = shape else {
        return shape;
    };

    for fetched in fetch {
        mark_field_path_fetched(&mut fields, &fetched.segments);
    }

    ResponseShape::Object { fields, open }
}

fn mark_field_path_fetched(fields: &mut BTreeMap<String, FieldShape>, segments: &[String]) {
    let Some((first, rest)) = segments.split_first() else {
        return;
    };

    let Some(field) = fields.get_mut(first) else {
        return;
    };
    if rest.is_empty() {
        field.materialized_by_fetch = true;
        return;
    }

    let ResponseShape::Object {
        fields: child_fields,
        ..
    } = &mut field.shape
    else {
        return;
    };
    mark_field_path_fetched(child_fields, rest);
}

fn object_shape_for_all_fields(table: &crate::schema::TableDef) -> ResponseShape {
    object_shape_for_field_prefix(table, &[])
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
        let Some(field_shape) = field_shape_for_path(table, path) else {
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

        if let Some(alias) = alias {
            fields.insert(alias.clone(), field_shape);
        } else {
            insert_field_shape_at_path(&mut fields, &path.segments, field_shape);
        }
    }

    ResponseShape::Object {
        fields,
        open: false,
    }
}

fn field_shape_for_path(table: &crate::schema::TableDef, path: &FieldPath) -> Option<FieldShape> {
    let has_descendants = table.fields.values().any(|field| {
        field.path.len() > path.segments.len() && field.path.starts_with(&path.segments)
    });

    if has_descendants {
        return Some(FieldShape {
            shape: object_shape_for_field_prefix(table, &path.segments),
            kind: exact_field_def_for_path(table, path).and_then(|field| field.kind.clone()),
            span: path.span.clone(),
            materialized_by_fetch: false,
            partial: Vec::new(),
        });
    }

    exact_field_def_for_path(table, path)
        .map(|field| field_shape_from_schema_field(field, path.span.clone()))
}

fn object_shape_for_field_prefix(
    table: &crate::schema::TableDef,
    prefix: &[String],
) -> ResponseShape {
    let mut fields = BTreeMap::new();

    for field in table.fields.values() {
        if field.path.len() <= prefix.len() || !field.path.starts_with(prefix) {
            continue;
        }

        let segment = field.path[prefix.len()].clone();
        if fields.contains_key(&segment) {
            continue;
        }

        let child_prefix: Vec<_> = prefix
            .iter()
            .cloned()
            .chain(std::iter::once(segment.clone()))
            .collect();
        let has_descendants = table.fields.values().any(|candidate| {
            candidate.path.len() > child_prefix.len() && candidate.path.starts_with(&child_prefix)
        });

        let field_shape = if has_descendants {
            FieldShape {
                shape: object_shape_for_field_prefix(table, &child_prefix),
                kind: None,
                span: field.name_span.clone(),
                materialized_by_fetch: false,
                partial: Vec::new(),
            }
        } else {
            field_shape_from_schema_field(field, field.name_span.clone())
        };
        fields.insert(segment, field_shape);
    }

    ResponseShape::Object {
        fields,
        open: false,
    }
}

fn insert_field_shape_at_path(
    fields: &mut BTreeMap<String, FieldShape>,
    segments: &[String],
    field_shape: FieldShape,
) {
    let Some((first, rest)) = segments.split_first() else {
        return;
    };

    if rest.is_empty() {
        fields.insert(first.clone(), field_shape);
        return;
    }

    let parent = fields.entry(first.clone()).or_insert_with(|| FieldShape {
        shape: ResponseShape::Object {
            fields: BTreeMap::new(),
            open: false,
        },
        kind: None,
        span: field_shape.span.clone(),
        materialized_by_fetch: false,
        partial: Vec::new(),
    });

    let ResponseShape::Object {
        fields: child_fields,
        ..
    } = &mut parent.shape
    else {
        return;
    };
    insert_field_shape_at_path(child_fields, rest, field_shape);
}

fn value_projection_shape(ir: &SelectIr, table: &crate::schema::TableDef) -> Option<ResponseShape> {
    let [SelectProjection::Field {
        path, value: true, ..
    }] = ir.projections.as_slice()
    else {
        return None;
    };
    let field = exact_field_def_for_path(table, path)?;
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

fn table_references_after_keyword<'tree>(
    node: Node<'tree>,
    source: &'tree str,
    keyword: &str,
    statement: &'static str,
) -> Vec<TableReference<'tree>> {
    fn visit<'tree>(
        node: Node<'tree>,
        source: &'tree str,
        keyword: &str,
        statement: &'static str,
        saw_keyword: &mut bool,
        references: &mut Vec<TableReference<'tree>>,
    ) {
        let text = node_text(node, source);
        if node.kind() == "Keyword" {
            *saw_keyword = text.eq_ignore_ascii_case(keyword);
        } else if *saw_keyword && is_identifier_like(node) {
            references.push(TableReference {
                name: table_name_from_node_text(text),
                node,
                statement,
            });
            *saw_keyword = false;
        } else if *saw_keyword && (!node.is_named() || is_data_or_modifier_clause(node)) {
            *saw_keyword = false;
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            visit(child, source, keyword, statement, saw_keyword, references);
        }
    }

    let mut saw_keyword = false;
    let mut references = Vec::new();
    visit(
        node,
        source,
        keyword,
        statement,
        &mut saw_keyword,
        &mut references,
    );
    references
}

fn statement_kind(node: Node<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "AlterStatement" => Some("alter".into()),
        "BeginStatement" => Some("begin".into()),
        "BreakStatement" => Some("break".into()),
        "CancelStatement" => Some("cancel".into()),
        "CommitStatement" => Some("commit".into()),
        "ContinueStatement" => Some("continue".into()),
        "CreateStatement" => Some("create".into()),
        "DefineStatement" => define_statement_kind(node, source),
        "DeleteStatement" => Some("delete".into()),
        "ForStatement" => Some("for".into()),
        "IfElseStatement" => Some("if_else".into()),
        "InfoForStatement" => Some("info_for".into()),
        "InsertStatement" => Some("insert".into()),
        "KillStatement" => Some("kill".into()),
        "LetStatement" => Some("let".into()),
        "LiveSelectStatement" => Some("live_select".into()),
        "OptionStatement" => Some("option".into()),
        "RebuildStatement" => Some("rebuild".into()),
        "RelateStatement" => Some("relate".into()),
        "RemoveStatement" => Some("remove".into()),
        "ReturnStatement" => Some("return".into()),
        "SelectStatement" => Some("select".into()),
        "ShowStatement" => Some("show".into()),
        "SleepStatement" => Some("sleep".into()),
        "ThrowStatement" => Some("throw".into()),
        "UpdateStatement" => Some("update".into()),
        "UpsertStatement" => Some("upsert".into()),
        "UseStatement" => Some("use".into()),
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
