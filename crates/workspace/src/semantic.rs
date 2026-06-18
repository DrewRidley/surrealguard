use std::collections::BTreeMap;

use surrealguard_diagnostics::{Finding, FindingCode, Severity};
use surrealguard_syntax::parse::ParsedSource;
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use surrealdb_types::Kind;
use tree_sitter::Node;

use crate::analysis::{ParamInference, SelectModifierAnalysis, StatementAnalysis};
use crate::expression::{infer_expression_fact, ExpressionFact, ExpressionValueClass};
use crate::response_shape::{FieldShape, PartialReason, ResponseShape};
use crate::schema::SchemaIndex;
use crate::select_ir::{
    select_ir_from_statement, FieldPath, GraphDirection, SelectIr, SelectModifier, SelectProjection,
};
use crate::statement_env::StatementEnv;

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

#[derive(Clone, Debug, PartialEq, Eq)]
struct LetVariableFact {
    kind: Option<Kind>,
    shape: Option<ResponseShape>,
}

pub fn analyze_parsed_source(parsed: &ParsedSource) -> SemanticOutput {
    if !parsed.syntax_diagnostics().is_empty() {
        return SemanticOutput::default();
    }

    let mut env = StatementEnv::default();
    analyze_statement_sequence(parsed.tree().root_node(), parsed, &mut env)
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

        let mut env = StatementEnv::default();
        collect_mutation_field_diagnostics_with_env(
            parsed.tree().root_node(),
            parsed,
            schema,
            &mut env,
            &mut diagnostics,
        );
    }

    diagnostics
}

pub fn validate_if_conditions(parsed_sources: &[ParsedSource]) -> Vec<Finding> {
    let mut diagnostics = Vec::new();

    for parsed in parsed_sources {
        if !parsed.syntax_diagnostics().is_empty() {
            continue;
        }

        let mut env = StatementEnv::default();
        collect_if_condition_diagnostics_with_env(
            parsed.tree().root_node(),
            parsed,
            &mut env,
            &mut diagnostics,
        );
    }

    diagnostics
}

pub fn validate_function_calls(
    parsed_sources: &[ParsedSource],
    schema: &SchemaIndex,
) -> Vec<Finding> {
    let mut diagnostics = Vec::new();

    for parsed in parsed_sources {
        if !parsed.syntax_diagnostics().is_empty() {
            continue;
        }

        let mut env = StatementEnv::default();
        collect_function_call_diagnostics_with_env(
            parsed.tree().root_node(),
            parsed,
            schema,
            &mut env,
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
        let mut env = StatementEnv::default();
        collect_select_response_shapes_with_env(
            parsed.tree().root_node(),
            parsed,
            schema,
            &mut env,
            &mut shapes,
        );
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
        let mut env = StatementEnv::default();
        let output = analyze_statement_sequence(parsed.tree().root_node(), parsed, &mut env);
        for statement in output.statements {
            if matches!(statement.kind.as_str(), "if_else" | "return") {
                if let Some(shape) = statement.response_shape {
                    shapes.push((statement.span, shape));
                }
            }
        }
        collect_mutation_response_shapes(parsed.tree().root_node(), parsed, schema, &mut shapes);
    }

    shapes
}

fn collect_mutation_response_shapes(
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
        collect_mutation_response_shapes(child, parsed, schema, shapes);
    }
}

fn merge_response_shapes(shapes: Vec<ResponseShape>) -> ResponseShape {
    let mut variants = Vec::new();
    for shape in shapes {
        if !variants.contains(&shape) {
            variants.push(shape);
        }
    }

    match variants.len() {
        0 => ResponseShape::Unknown {
            reason: PartialReason::Unresolved,
        },
        1 => variants.pop().expect("one response-shape variant"),
        _ => ResponseShape::Union { variants },
    }
}

fn response_shape_for_return(
    node: Node<'_>,
    parsed: &ParsedSource,
    let_variables: &BTreeMap<String, LetVariableFact>,
) -> ResponseShape {
    let Some(value) = return_value_node(node) else {
        return ResponseShape::Unknown {
            reason: PartialReason::Unresolved,
        };
    };
    if value.kind() == "VariableName" {
        let name = param_name(node_text(value, parsed.text()));
        if let Some(variable) = let_variables.get(&name) {
            if let Some(shape) = variable.shape.clone() {
                return shape;
            }
            if let Some(kind) = variable.kind.clone() {
                return ResponseShape::Value { kind };
            }
        }
    }

    infer_expression_fact_with_let_variables(value, parsed, None, let_variables)
        .shape
        .unwrap_or(ResponseShape::Unknown {
            reason: PartialReason::Unresolved,
        })
}

fn return_value_node(statement: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = statement.walk();
    statement
        .children(&mut cursor)
        .filter(|child| child.is_named() && child.kind() != "Keyword")
        .last()
}

fn response_shape_for_mutation(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
) -> ResponseShape {
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

    match mutation_return_mode(node, parsed) {
        MutationReturnMode::None => ResponseShape::Array {
            element: Box::new(ResponseShape::Unknown {
                reason: PartialReason::UnsupportedSyntax("RETURN NONE".into()),
            }),
            max_len: Some(0),
        },
        MutationReturnMode::Diff => mutation_return_diff_shape(node, parsed),
        MutationReturnMode::Fields => mutation_return_fields_shape(node, parsed, table),
        MutationReturnMode::Rows => {
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

fn mutation_return_fields_shape(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
) -> ResponseShape {
    let Some(return_clause) = find_descendant_kind(node, "ReturnClause") else {
        return ResponseShape::Unknown {
            reason: PartialReason::Unresolved,
        };
    };
    let mut fields = BTreeMap::new();
    collect_return_field_shapes(return_clause, parsed, table, &mut fields);
    if fields.is_empty() {
        return ResponseShape::Unknown {
            reason: PartialReason::Unresolved,
        };
    }

    ResponseShape::Array {
        element: Box::new(ResponseShape::Object {
            fields,
            open: false,
        }),
        max_len: None,
    }
}

fn collect_return_field_shapes(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    fields: &mut BTreeMap<String, FieldShape>,
) {
    if is_row_context_field_path_node(node) {
        let path = field_path_from_node(node, parsed);
        if let Some(field_shape) = field_shape_for_path(table, &path) {
            insert_field_shape_at_path(fields, &path.segments, field_shape);
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() && !matches!(child.kind(), "Keyword" | "Literal") {
            collect_return_field_shapes(child, parsed, table, fields);
        }
    }
}

fn mutation_return_diff_shape(node: Node<'_>, parsed: &ParsedSource) -> ResponseShape {
    let span = find_descendant_kind(node, "ReturnClause")
        .map(|clause| node_span(clause, parsed.source_id().clone()))
        .unwrap_or_else(|| node_span(node, parsed.source_id().clone()));
    let mut fields = BTreeMap::new();
    for (name, kind) in [
        ("op", Kind::String),
        ("path", Kind::String),
        ("value", Kind::Any),
    ] {
        fields.insert(
            name.to_string(),
            FieldShape {
                shape: ResponseShape::Value { kind: kind.clone() },
                kind: Some(kind),
                span: span.clone(),
                materialized_by_fetch: false,
                partial: Vec::new(),
            },
        );
    }

    ResponseShape::Array {
        element: Box::new(ResponseShape::Array {
            element: Box::new(ResponseShape::Object {
                fields,
                open: false,
            }),
            max_len: None,
        }),
        max_len: None,
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

fn collect_select_response_shapes_with_env(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    env: &mut StatementEnv,
    shapes: &mut Vec<(SourceSpan, ResponseShape)>,
) {
    match node.kind() {
        "LetStatement" => {
            define_let_from_statement(node, parsed, env);
            return;
        }
        "Block" => {
            let mut child_env = env.fork_child_scope();
            collect_select_response_shape_children_with_env(
                node,
                parsed,
                schema,
                &mut child_env,
                shapes,
            );
            return;
        }
        "SelectStatement" => {
            let ir = select_ir_from_statement(node, parsed);
            let let_variables = let_variable_facts_from_env(env);
            shapes.push((
                node_span(node, parsed.source_id().clone()),
                response_shape_for_select(&ir, parsed, schema, &let_variables),
            ));
            return;
        }
        _ => {}
    }

    collect_select_response_shape_children_with_env(node, parsed, schema, env, shapes);
}

fn collect_select_response_shape_children_with_env(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    env: &mut StatementEnv,
    shapes: &mut Vec<(SourceSpan, ResponseShape)>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_select_response_shapes_with_env(child, parsed, schema, env, shapes);
    }
}

pub fn analyze_statement_sequence(
    node: Node<'_>,
    parsed: &ParsedSource,
    env: &mut StatementEnv,
) -> SemanticOutput {
    let mut output = SemanticOutput::default();
    analyze_statement_sequence_into(node, parsed, env, &mut output);
    output
        .inferred_params
        .sort_by(|left, right| left.name.cmp(&right.name));
    output
}

fn analyze_statement_sequence_into(
    node: Node<'_>,
    parsed: &ParsedSource,
    env: &mut StatementEnv,
    output: &mut SemanticOutput,
) {
    if let Some(kind) = statement_kind(node, parsed.text()) {
        let statement_index = output.statements.len();
        output.statements.push(StatementAnalysis {
            span: node_span(node, parsed.source_id().clone()),
            kind,
            response_shape: None,
            select_modifiers: select_modifier_analysis_for_node(node, parsed),
        });
        output.statements[statement_index].response_shape =
            analyze_statement_effects(node, parsed, env, output);
        return;
    }

    if node.kind() == "Block" {
        let mut child_env = env.fork_child_scope();
        let mut child_output = SemanticOutput::default();
        analyze_statement_children(node, parsed, &mut child_env, &mut child_output);
        merge_param_inferences(&mut output.inferred_params, child_output.inferred_params);
        return;
    }

    analyze_statement_children(node, parsed, env, output);
}

fn analyze_statement_children(
    node: Node<'_>,
    parsed: &ParsedSource,
    env: &mut StatementEnv,
    output: &mut SemanticOutput,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        analyze_statement_sequence_into(child, parsed, env, output);
    }
}

fn analyze_statement_effects(
    node: Node<'_>,
    parsed: &ParsedSource,
    env: &mut StatementEnv,
    output: &mut SemanticOutput,
) -> Option<ResponseShape> {
    match node.kind() {
        "LetStatement" => {
            if let Some(value_node) = let_value_node(node) {
                collect_expression_params_with_env(value_node, parsed, env, output);
            }
            define_let_from_statement(node, parsed, env);
            None
        }
        "ReturnStatement" => {
            let let_variables = let_variable_facts_from_env(env);
            let shape = response_shape_for_return(node, parsed, &let_variables);
            if let Some(value_node) = return_value_node(node) {
                collect_expression_params_with_env(value_node, parsed, env, output);
            }
            Some(shape)
        }
        "IfElseStatement" => Some(analyze_if_else_statement_effects(node, parsed, env, output)),
        _ => {
            collect_expression_params_with_env(node, parsed, env, output);
            None
        }
    }
}

fn analyze_if_else_statement_effects(
    node: Node<'_>,
    parsed: &ParsedSource,
    env: &mut StatementEnv,
    output: &mut SemanticOutput,
) -> ResponseShape {
    let mut branch_shapes = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        analyze_if_else_child(child, parsed, env, output, &mut branch_shapes);
    }
    merge_response_shapes(branch_shapes)
}

fn analyze_if_else_child(
    node: Node<'_>,
    parsed: &ParsedSource,
    env: &mut StatementEnv,
    output: &mut SemanticOutput,
    branch_shapes: &mut Vec<ResponseShape>,
) {
    if node.kind() == "Block" {
        let mut child_env = env.fork_child_scope();
        let mut child_output = SemanticOutput::default();
        analyze_statement_children(node, parsed, &mut child_env, &mut child_output);
        collect_output_response_shapes(&child_output, branch_shapes);
        merge_param_inferences(&mut output.inferred_params, child_output.inferred_params);
        return;
    }

    if node.kind() == "IfElseStatement" {
        let mut child_env = env.fork_child_scope();
        let child_output = analyze_statement_sequence(node, parsed, &mut child_env);
        collect_output_response_shapes(&child_output, branch_shapes);
        merge_param_inferences(&mut output.inferred_params, child_output.inferred_params);
        return;
    }

    if node.kind() == "VariableName" {
        let name = param_name(node_text(node, parsed.text()));
        if env.let_fact(&name).is_none() {
            let span = node_span(node, parsed.source_id().clone());
            env.record_param_use(name.clone(), span.clone());
            record_param_output(&mut output.inferred_params, name, span);
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        analyze_if_else_child(child, parsed, env, output, branch_shapes);
    }
}

fn collect_output_response_shapes(output: &SemanticOutput, shapes: &mut Vec<ResponseShape>) {
    shapes.extend(
        output
            .statements
            .iter()
            .filter_map(|statement| statement.response_shape.clone()),
    );
}

fn collect_expression_params_with_env(
    node: Node<'_>,
    parsed: &ParsedSource,
    env: &mut StatementEnv,
    output: &mut SemanticOutput,
) {
    if node.kind() == "LetStatement" {
        if let Some(value_node) = let_value_node(node) {
            collect_expression_params_with_env(value_node, parsed, env, output);
        }
        if let Some(name_node) = let_variable_name_node(node) {
            let name = param_name(node_text(name_node, parsed.text()));
            let fact = infer_expression_fact(value_node_or_unknown(node), parsed, None);
            env.define_let(name, fact);
        }
        return;
    }

    if node.kind() == "Block" {
        let mut child_env = env.fork_child_scope();
        let child_output = analyze_statement_sequence(node, parsed, &mut child_env);
        merge_param_inferences(&mut output.inferred_params, child_output.inferred_params);
        return;
    }

    if node.kind() == "VariableName" {
        let name = param_name(node_text(node, parsed.text()));
        if env.let_fact(&name).is_none() {
            let span = node_span(node, parsed.source_id().clone());
            env.record_param_use(name.clone(), span.clone());
            record_param_output(&mut output.inferred_params, name, span);
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_expression_params_with_env(child, parsed, env, output);
    }
}

fn record_param_output(target: &mut Vec<ParamInference>, name: String, span: SourceSpan) {
    if let Some(existing) = target.iter_mut().find(|existing| existing.name == name) {
        existing.spans.push(span);
    } else {
        target.push(ParamInference {
            name,
            kind: None,
            required: true,
            spans: vec![span],
        });
    }
}

fn merge_param_inferences(target: &mut Vec<ParamInference>, incoming: Vec<ParamInference>) {
    for param in incoming {
        if let Some(existing) = target
            .iter_mut()
            .find(|existing| existing.name == param.name)
        {
            if existing.kind.is_none() {
                existing.kind = param.kind.clone();
            }
            existing.required |= param.required;
            existing.spans.extend(param.spans);
        } else {
            target.push(param);
        }
    }
}

fn value_node_or_unknown(node: Node<'_>) -> Node<'_> {
    let_value_node(node).unwrap_or(node)
}

fn let_variable_facts_from_env(env: &StatementEnv) -> BTreeMap<String, LetVariableFact> {
    env.let_facts()
        .iter()
        .map(|(name, fact)| {
            (
                name.clone(),
                LetVariableFact {
                    kind: fact.kind.clone(),
                    shape: fact.shape.clone(),
                },
            )
        })
        .collect()
}

fn let_variable_name_node(statement: Node<'_>) -> Option<Node<'_>> {
    find_descendant_kind(statement, "VariableName")
}

fn let_value_node(statement: Node<'_>) -> Option<Node<'_>> {
    let name = let_variable_name_node(statement)?;
    let mut cursor = statement.walk();
    let found = statement
        .children(&mut cursor)
        .filter(|child| child.is_named() && child.start_byte() > name.end_byte())
        .last();
    found
}

fn single_named_child<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    let mut children = node.named_children(&mut cursor);
    let first = children.next()?;
    if children.next().is_none() {
        Some(first)
    } else {
        None
    }
}

fn infer_expression_fact_with_let_variables(
    node: Node<'_>,
    parsed: &ParsedSource,
    row_table: Option<&crate::schema::TableDef>,
    let_variables: &BTreeMap<String, LetVariableFact>,
) -> ExpressionFact {
    if matches!(node.kind(), "Fields" | "Predicate") {
        if let Some(child) = single_named_child(node) {
            return infer_expression_fact_with_let_variables(
                child,
                parsed,
                row_table,
                let_variables,
            );
        }
    }

    if node.kind() == "VariableName" {
        let name = param_name(node_text(node, parsed.text()));
        if let Some(variable) = let_variables.get(&name) {
            let mut fact = ExpressionFact::new(
                node_span(node, parsed.source_id().clone()),
                ExpressionValueClass::Variable,
            );
            fact.kind = variable.kind.clone();
            fact.shape = variable.shape.clone();
            return fact;
        }
    }
    if node.kind() == "BinaryExpression" {
        return infer_binary_expression_fact_with_let_variables(
            node,
            parsed,
            row_table,
            let_variables,
        );
    }
    infer_expression_fact(node, parsed, row_table)
}

fn infer_binary_expression_fact_with_let_variables(
    node: Node<'_>,
    parsed: &ParsedSource,
    row_table: Option<&crate::schema::TableDef>,
    let_variables: &BTreeMap<String, LetVariableFact>,
) -> ExpressionFact {
    let Some((left, operator, right)) = binary_expression_parts(node) else {
        return ExpressionFact::new(
            node_span(node, parsed.source_id().clone()),
            ExpressionValueClass::Unknown,
        )
        .with_partial(PartialReason::UnsupportedSyntax("BinaryExpression".into()));
    };

    let left_fact =
        infer_expression_fact_with_let_variables(left, parsed, row_table, let_variables);
    let right_fact =
        infer_expression_fact_with_let_variables(right, parsed, row_table, let_variables);
    let operator_text = node_text(operator, parsed.text()).trim();
    let mut fact = ExpressionFact::new(
        node_span(node, parsed.source_id().clone()),
        ExpressionValueClass::Unknown,
    );
    fact.dependencies
        .field_paths
        .extend(left_fact.dependencies.field_paths);
    fact.dependencies
        .field_paths
        .extend(right_fact.dependencies.field_paths);
    fact.dependencies
        .variables
        .extend(left_fact.dependencies.variables);
    fact.dependencies
        .variables
        .extend(right_fact.dependencies.variables);
    fact.dependencies
        .params
        .extend(left_fact.dependencies.params);
    fact.dependencies
        .params
        .extend(right_fact.dependencies.params);

    if let (Some(left_kind), Some(right_kind)) = (&left_fact.kind, &right_fact.kind) {
        if let Some(kind) = binary_expression_result_kind(operator_text, left_kind, right_kind) {
            return fact
                .with_kind(kind.clone())
                .with_shape(ResponseShape::Value { kind });
        }
    }

    fact.with_partial(PartialReason::UnsupportedSyntax("BinaryExpression".into()))
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

fn collect_function_call_diagnostics_with_env(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    env: &mut StatementEnv,
    diagnostics: &mut Vec<Finding>,
) {
    match node.kind() {
        "LetStatement" => {
            let let_variables = let_variable_facts_from_env(env);
            if let Some(value_node) = let_value_node(node) {
                validate_function_calls_in_node(
                    value_node,
                    parsed,
                    None,
                    &let_variables,
                    diagnostics,
                );
            }
            define_let_from_statement(node, parsed, env);
            return;
        }
        "Block" => {
            let mut child_env = env.fork_child_scope();
            collect_function_call_children_with_env(
                node,
                parsed,
                schema,
                &mut child_env,
                diagnostics,
            );
            return;
        }
        "SelectStatement" => {
            let ir = select_ir_from_statement(node, parsed);
            let table =
                resolved_select_table_name(&ir, schema).and_then(|name| schema.tables.get(&name));
            let let_variables = let_variable_facts_from_env(env);
            validate_function_calls_in_node(node, parsed, table, &let_variables, diagnostics);
            return;
        }
        "ReturnStatement" => {
            let let_variables = let_variable_facts_from_env(env);
            if let Some(value_node) = return_value_node(node) {
                validate_function_calls_in_node(
                    value_node,
                    parsed,
                    None,
                    &let_variables,
                    diagnostics,
                );
            }
            return;
        }
        _ => {}
    }

    collect_function_call_children_with_env(node, parsed, schema, env, diagnostics);
}

fn collect_function_call_children_with_env(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    env: &mut StatementEnv,
    diagnostics: &mut Vec<Finding>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_function_call_diagnostics_with_env(child, parsed, schema, env, diagnostics);
    }
}

fn collect_if_condition_diagnostics_with_env(
    node: Node<'_>,
    parsed: &ParsedSource,
    env: &mut StatementEnv,
    diagnostics: &mut Vec<Finding>,
) {
    match node.kind() {
        "LetStatement" => {
            define_let_from_statement(node, parsed, env);
            return;
        }
        "IfElseStatement" => {
            let let_variables = let_variable_facts_from_env(env);
            for condition in if_condition_nodes(node) {
                validate_if_condition(condition, parsed, &let_variables, diagnostics);
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                collect_if_condition_child_with_env(child, parsed, env, diagnostics);
            }
            return;
        }
        "Block" => {
            let mut child_env = env.fork_child_scope();
            collect_if_condition_statement_children(node, parsed, &mut child_env, diagnostics);
            return;
        }
        _ => {}
    }

    collect_if_condition_statement_children(node, parsed, env, diagnostics);
}

fn collect_if_condition_statement_children(
    node: Node<'_>,
    parsed: &ParsedSource,
    env: &mut StatementEnv,
    diagnostics: &mut Vec<Finding>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_if_condition_diagnostics_with_env(child, parsed, env, diagnostics);
    }
}

fn collect_if_condition_child_with_env(
    node: Node<'_>,
    parsed: &ParsedSource,
    env: &mut StatementEnv,
    diagnostics: &mut Vec<Finding>,
) {
    if node.kind() == "Block" {
        let mut child_env = env.fork_child_scope();
        collect_if_condition_statement_children(node, parsed, &mut child_env, diagnostics);
        return;
    }

    collect_if_condition_diagnostics_with_env(node, parsed, env, diagnostics);
}

fn define_let_from_statement(node: Node<'_>, parsed: &ParsedSource, env: &mut StatementEnv) {
    if let (Some(name_node), Some(value_node)) =
        (let_variable_name_node(node), let_value_node(node))
    {
        let name = param_name(node_text(name_node, parsed.text()));
        let let_variables = let_variable_facts_from_env(env);
        let fact =
            infer_expression_fact_with_let_variables(value_node, parsed, None, &let_variables);
        env.define_let(name, fact);
    }
}

fn validate_if_condition(
    node: Node<'_>,
    parsed: &ParsedSource,
    let_variables: &BTreeMap<String, LetVariableFact>,
    diagnostics: &mut Vec<Finding>,
) {
    let fact = infer_expression_fact_with_let_variables(node, parsed, None, let_variables);
    let Some(kind) = fact.kind else {
        return;
    };
    if matches!(kind, Kind::Bool) {
        return;
    }

    diagnostics.push(Finding::new(
        fact.span,
        FindingCode::type_error(2006),
        Severity::Error,
        format!(
            "IF condition has type `{}`, expected `bool`",
            kind_name(&kind)
        ),
    ));
}

fn if_condition_nodes<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let Some(branch) =
        direct_child_of_kind(node, "Modern").or_else(|| direct_child_of_kind(node, "Legacy"))
    else {
        return Vec::new();
    };

    let mut conditions = Vec::new();
    let mut cursor = branch.walk();
    for child in branch.named_children(&mut cursor) {
        if !matches!(child.kind(), "Keyword" | "Block" | "SubQuery") {
            conditions.push(child);
        }
    }
    conditions
}

fn direct_child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    let child = node
        .children(&mut cursor)
        .find(|child| child.kind() == kind);
    child
}

fn validate_function_calls_in_node(
    node: Node<'_>,
    parsed: &ParsedSource,
    row_table: Option<&crate::schema::TableDef>,
    let_variables: &BTreeMap<String, LetVariableFact>,
    diagnostics: &mut Vec<Finding>,
) {
    if node.kind() == "FunctionCall" {
        validate_function_call(node, parsed, row_table, let_variables, diagnostics);
    }
    if node.kind() == "BinaryExpression" {
        validate_binary_expression(node, parsed, row_table, let_variables, diagnostics);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        validate_function_calls_in_node(child, parsed, row_table, let_variables, diagnostics);
    }
}

fn validate_function_call(
    node: Node<'_>,
    parsed: &ParsedSource,
    row_table: Option<&crate::schema::TableDef>,
    let_variables: &BTreeMap<String, LetVariableFact>,
    diagnostics: &mut Vec<Finding>,
) {
    let Some(name) = function_call_name(node, parsed) else {
        return;
    };
    let args = function_call_args(node);
    let Some(signature) = function_signature(&name) else {
        diagnostics.push(Finding::new(
            node_span(node, parsed.source_id().clone()),
            FindingCode::type_error(2002),
            Severity::Error,
            format!("unknown function `{name}`"),
        ));
        return;
    };

    if args.len() != signature.args.len() {
        diagnostics.push(Finding::new(
            node_span(node, parsed.source_id().clone()),
            FindingCode::type_error(2003),
            Severity::Error,
            format!(
                "function `{name}` expects {} {}, got {}",
                signature.args.len(),
                pluralize_word("argument", signature.args.len()),
                args.len()
            ),
        ));
        return;
    }

    for (index, (arg, expected)) in args.iter().zip(signature.args.iter()).enumerate() {
        let fact = infer_expression_fact_with_let_variables(*arg, parsed, row_table, let_variables);
        let Some(actual) = fact.kind else {
            continue;
        };
        if function_arg_kind_matches(&actual, expected) {
            continue;
        }
        diagnostics.push(Finding::new(
            fact.span,
            FindingCode::type_error(2004),
            Severity::Error,
            format!(
                "argument {} to `{name}` has type `{}`, expected `{}`",
                index + 1,
                kind_name(&actual),
                function_arg_kind_name(expected)
            ),
        ));
    }
}

fn validate_binary_expression(
    node: Node<'_>,
    parsed: &ParsedSource,
    row_table: Option<&crate::schema::TableDef>,
    let_variables: &BTreeMap<String, LetVariableFact>,
    diagnostics: &mut Vec<Finding>,
) {
    let Some((left, operator, right)) = binary_expression_parts(node) else {
        return;
    };
    let left_fact =
        infer_expression_fact_with_let_variables(left, parsed, row_table, let_variables);
    let right_fact =
        infer_expression_fact_with_let_variables(right, parsed, row_table, let_variables);
    let (Some(left_kind), Some(right_kind)) = (&left_fact.kind, &right_fact.kind) else {
        return;
    };
    let operator_text = node_text(operator, parsed.text()).trim();
    if binary_expression_result_kind(operator_text, left_kind, right_kind).is_some() {
        return;
    }

    diagnostics.push(Finding::new(
        node_span(node, parsed.source_id().clone()),
        FindingCode::type_error(2005),
        Severity::Error,
        format!(
            "operator `{operator_text}` cannot combine `{}` and `{}`",
            kind_name(left_kind),
            kind_name(right_kind)
        ),
    ));
}

fn binary_expression_parts<'tree>(
    node: Node<'tree>,
) -> Option<(Node<'tree>, Node<'tree>, Node<'tree>)> {
    let mut cursor = node.walk();
    let children: Vec<_> = node
        .children(&mut cursor)
        .filter(|child| child.is_named())
        .collect();
    let operator_index = children
        .iter()
        .position(|child| child.kind() == "Operator")?;
    let left = children[..operator_index]
        .iter()
        .rev()
        .copied()
        .find(|child| child.kind() != "Operator")?;
    let right = children[operator_index + 1..]
        .iter()
        .copied()
        .find(|child| child.kind() != "Operator")?;
    Some((left, children[operator_index], right))
}

fn binary_expression_result_kind(operator: &str, left: &Kind, right: &Kind) -> Option<Kind> {
    match operator {
        "+" if matches!(left, Kind::String) && matches!(right, Kind::String) => Some(Kind::String),
        "+" | "-" | "*" | "/" if is_numeric_kind(left) && is_numeric_kind(right) => {
            if matches!(left, Kind::Float) || matches!(right, Kind::Float) {
                Some(Kind::Float)
            } else {
                Some(Kind::Int)
            }
        }
        _ => None,
    }
}

fn is_numeric_kind(kind: &Kind) -> bool {
    matches!(kind, Kind::Int | Kind::Float | Kind::Number)
}

#[derive(Clone, Debug)]
struct FunctionSignature {
    args: Vec<FunctionArgKind>,
    return_kind: Kind,
}

#[derive(Clone, Debug)]
enum FunctionArgKind {
    Exact(Kind),
    Array,
}

fn function_signature(name: &str) -> Option<FunctionSignature> {
    match name {
        "string::len" => Some(FunctionSignature {
            args: vec![FunctionArgKind::Exact(Kind::String)],
            return_kind: Kind::Int,
        }),
        "array::len" => Some(FunctionSignature {
            args: vec![FunctionArgKind::Array],
            return_kind: Kind::Int,
        }),
        "count" => Some(FunctionSignature {
            args: Vec::new(),
            return_kind: Kind::Int,
        }),
        _ => None,
    }
}

fn function_arg_kind_matches(actual: &Kind, expected: &FunctionArgKind) -> bool {
    match expected {
        FunctionArgKind::Exact(expected) => kind_is_assignable_to(actual, expected),
        FunctionArgKind::Array => matches!(actual, Kind::Array(_, _) | Kind::Set(_, _)),
    }
}

fn function_arg_kind_name(expected: &FunctionArgKind) -> &'static str {
    match expected {
        FunctionArgKind::Exact(kind) => kind_name(kind),
        FunctionArgKind::Array => "array",
    }
}

fn pluralize_word(word: &str, count: usize) -> String {
    if count == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

fn function_call_name(node: Node<'_>, parsed: &ParsedSource) -> Option<String> {
    let name = first_child_kind(node, "FunctionName")?;
    Some(node_text(name, parsed.text()).trim().to_string())
}

fn function_call_args(node: Node<'_>) -> Vec<Node<'_>> {
    let Some(arguments) = first_child_kind(node, "ArgumentList") else {
        return Vec::new();
    };
    let mut cursor = arguments.walk();
    arguments
        .children(&mut cursor)
        .filter(|child| child.is_named())
        .collect()
}

fn first_child_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| child.kind() == kind);
    found
}

fn exact_node_for_span<'tree>(node: Node<'tree>, span: &SourceSpan) -> Option<Node<'tree>> {
    let start = span.range().start() as usize;
    let end = span.range().end() as usize;
    if node.start_byte() == start && node.end_byte() == end {
        return Some(node);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.start_byte() <= start && child.end_byte() >= end {
            if let Some(found) = exact_node_for_span(child, span) {
                return Some(found);
            }
        }
    }
    None
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

fn collect_mutation_field_diagnostics_with_env(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    env: &mut StatementEnv,
    diagnostics: &mut Vec<Finding>,
) {
    match node.kind() {
        "LetStatement" => {
            define_let_from_statement(node, parsed, env);
            return;
        }
        "Block" => {
            let mut child_env = env.fork_child_scope();
            collect_mutation_field_children_with_env(
                node,
                parsed,
                schema,
                &mut child_env,
                diagnostics,
            );
            return;
        }
        _ => {}
    }

    let let_variables = let_variable_facts_from_env(env);
    validate_mutation_fields_for_statement(node, parsed, schema, &let_variables, diagnostics);
    collect_mutation_field_children_with_env(node, parsed, schema, env, diagnostics);
}

fn collect_mutation_field_children_with_env(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    env: &mut StatementEnv,
    diagnostics: &mut Vec<Finding>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_mutation_field_diagnostics_with_env(child, parsed, schema, env, diagnostics);
    }
}

fn validate_mutation_fields_for_statement(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    let_variables: &BTreeMap<String, LetVariableFact>,
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
    validate_mutation_value_assignability(node, parsed, table, let_variables, diagnostics);
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
    let_variables: &BTreeMap<String, LetVariableFact>,
    diagnostics: &mut Vec<Finding>,
) {
    validate_assignment_value_assignability(node, parsed, table, let_variables, diagnostics);
    validate_object_value_assignability(node, parsed, table, let_variables, diagnostics);
    validate_insert_tuple_value_assignability(node, parsed, table, let_variables, diagnostics);
}

fn validate_assignment_value_assignability(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    let_variables: &BTreeMap<String, LetVariableFact>,
    diagnostics: &mut Vec<Finding>,
) {
    if node.kind() == "FieldAssignment" {
        if let (Some(path), Some(value)) = (
            assignment_field_path(node, parsed),
            assignment_value_node(node),
        ) {
            validate_value_kind_for_field(path, value, parsed, table, let_variables, diagnostics);
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        validate_assignment_value_assignability(child, parsed, table, let_variables, diagnostics);
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
    let_variables: &BTreeMap<String, LetVariableFact>,
    diagnostics: &mut Vec<Finding>,
) {
    match node.kind() {
        "ContentClause" | "MergeClause" | "ReplaceClause" | "BulkInsert" => {
            validate_object_value_descendants(node, parsed, table, let_variables, diagnostics);
            return;
        }
        "InsertStatement" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "Object" || child.kind() == "BulkInsert" {
                    validate_object_value_descendants(
                        child,
                        parsed,
                        table,
                        let_variables,
                        diagnostics,
                    );
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        validate_object_value_assignability(child, parsed, table, let_variables, diagnostics);
    }
}

fn validate_object_value_descendants(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    let_variables: &BTreeMap<String, LetVariableFact>,
    diagnostics: &mut Vec<Finding>,
) {
    if node.kind() == "Object" {
        validate_object_property_values_on_table(
            node,
            parsed,
            table,
            let_variables,
            Vec::new(),
            diagnostics,
        );
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        validate_object_value_descendants(child, parsed, table, let_variables, diagnostics);
    }
}

fn validate_object_property_values_on_table(
    object: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    let_variables: &BTreeMap<String, LetVariableFact>,
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
                let_variables,
                segments,
                diagnostics,
            );
        } else if let Some(value) = object_property_value_node(object) {
            validate_value_kind_for_field(path, value, parsed, table, let_variables, diagnostics);
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
                let_variables,
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
    let_variables: &BTreeMap<String, LetVariableFact>,
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
            validate_value_kind_for_field(path, value, parsed, table, let_variables, diagnostics);
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
    let_variables: &BTreeMap<String, LetVariableFact>,
    diagnostics: &mut Vec<Finding>,
) {
    let Some(field) = exact_field_def_for_path(table, &path) else {
        return;
    };
    let Some(expected) = field.kind.clone() else {
        return;
    };
    let fact = infer_expression_fact_with_let_variables(value, parsed, Some(table), let_variables);
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
    collect_function_param_kind_inferences_in_node(node, parsed, inferences);

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

fn collect_function_param_kind_inferences_in_node(
    node: Node<'_>,
    parsed: &ParsedSource,
    inferences: &mut Vec<ParamKindInference>,
) {
    if node.kind() == "FunctionCall" {
        collect_function_param_kind_inferences(node, parsed, inferences);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_function_param_kind_inferences_in_node(child, parsed, inferences);
    }
}

fn collect_function_param_kind_inferences(
    node: Node<'_>,
    parsed: &ParsedSource,
    inferences: &mut Vec<ParamKindInference>,
) {
    let Some(name) = function_call_name(node, parsed) else {
        return;
    };
    let Some(signature) = function_signature(&name) else {
        return;
    };
    let args = function_call_args(node);
    if args.len() != signature.args.len() {
        return;
    }

    for (arg, expected) in args.iter().zip(signature.args.iter()) {
        if arg.kind() != "VariableName" {
            continue;
        }
        let Some(kind) = function_arg_param_kind(expected) else {
            continue;
        };
        inferences.push(ParamKindInference {
            source: parsed.source_id().clone(),
            name: param_name(node_text(*arg, parsed.text())),
            kind,
        });
    }
}

fn function_arg_param_kind(expected: &FunctionArgKind) -> Option<Kind> {
    match expected {
        FunctionArgKind::Exact(kind) => Some(kind.clone()),
        FunctionArgKind::Array => Some(Kind::Array(Box::new(Kind::Any), None)),
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

fn response_shape_for_select(
    ir: &SelectIr,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    let_variables: &BTreeMap<String, LetVariableFact>,
) -> ResponseShape {
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
    } else if let Some(value_shape) = value_projection_shape(ir, parsed, table, let_variables) {
        value_shape
    } else {
        object_shape_for_projected_fields(ir, parsed, table, let_variables)
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
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    let_variables: &BTreeMap<String, LetVariableFact>,
) -> ResponseShape {
    let mut fields = BTreeMap::new();

    for projection in &ir.projections {
        match projection {
            SelectProjection::Field { path, alias, .. } => {
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
            SelectProjection::Dynamic {
                span,
                alias,
                expression_kind,
                expression_text,
                ..
            } => {
                let key = alias.clone().unwrap_or_else(|| expression_text.clone());
                fields.insert(
                    key,
                    field_shape_for_dynamic_select_expression(
                        span.clone(),
                        expression_kind.as_deref(),
                        expression_text,
                        parsed,
                        table,
                        let_variables,
                    ),
                );
            }
            SelectProjection::Wildcard { .. } => {}
        }
    }

    ResponseShape::Object {
        fields,
        open: false,
    }
}

fn field_shape_for_dynamic_select_expression(
    span: SourceSpan,
    expression_kind: Option<&str>,
    expression_text: &str,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    let_variables: &BTreeMap<String, LetVariableFact>,
) -> FieldShape {
    let expression_fact = exact_node_for_span(parsed.tree().root_node(), &span).map(|node| {
        infer_expression_fact_with_let_variables(node, parsed, Some(table), let_variables)
    });
    let kind = match expression_kind {
        Some("BinaryExpression") => expression_fact.as_ref().and_then(|fact| fact.kind.clone()),
        Some("FunctionCall") => expression_fact
            .as_ref()
            .and_then(|fact| fact.kind.clone())
            .or_else(|| {
                function_name_from_expression_text(expression_text).and_then(|name| {
                    function_signature(name).map(|signature| signature.return_kind)
                })
            }),
        Some("Number") => {
            if expression_text.contains('.') {
                Some(Kind::Float)
            } else {
                Some(Kind::Int)
            }
        }
        Some("String") => Some(Kind::String),
        Some("Bool") => Some(Kind::Bool),
        _ => None,
    };
    let partial = if kind.is_none() {
        vec![PartialReason::UnsupportedSyntax(
            expression_kind.unwrap_or("expression").to_string(),
        )]
    } else {
        Vec::new()
    };
    let shape = match kind.clone() {
        Some(kind) => ResponseShape::Value { kind },
        None => ResponseShape::Unknown {
            reason: partial
                .first()
                .cloned()
                .unwrap_or(PartialReason::Unresolved),
        },
    };

    FieldShape {
        shape,
        kind,
        span,
        materialized_by_fetch: false,
        partial,
    }
}

fn function_name_from_expression_text(expression_text: &str) -> Option<&str> {
    expression_text.split_once('(').map(|(name, _)| name.trim())
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

fn value_projection_shape(
    ir: &SelectIr,
    _parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    _let_variables: &BTreeMap<String, LetVariableFact>,
) -> Option<ResponseShape> {
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

#[cfg(test)]
mod tests {
    use surrealdb_types::Kind;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    use super::*;
    use crate::statement_env::StatementEnv;

    #[test]
    fn analyze_statement_sequence_emits_statements_and_ordered_params() {
        let parsed = parse_source(
            SourceId::new("sequence-test"),
            "LET $known = $input; RETURN $known; RETURN $later;",
        )
        .expect("valid source parses");
        let mut env = StatementEnv::default();

        let output = analyze_statement_sequence(parsed.tree().root_node(), &parsed, &mut env);

        assert_eq!(
            output
                .statements
                .iter()
                .map(|statement| statement.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["let", "return", "return"]
        );
        assert_eq!(
            output
                .inferred_params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>(),
            vec!["input", "later"]
        );
    }

    #[test]
    fn analyze_statement_sequence_keeps_block_lets_local_for_params() {
        let parsed = parse_source(
            SourceId::new("sequence-test"),
            "LET $outer = 1; IF true { LET $inner = $outer; RETURN $inner; }; RETURN $inner;",
        )
        .expect("valid source parses");
        let mut env = StatementEnv::default();

        let output = analyze_statement_sequence(parsed.tree().root_node(), &parsed, &mut env);

        assert_eq!(
            output
                .inferred_params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>(),
            vec!["inner"]
        );
    }

    #[test]
    fn analyze_statement_sequence_records_let_expression_kind_in_env() {
        let parsed = parse_source(SourceId::new("sequence-test"), "LET $age = 42;")
            .expect("valid source parses");
        let mut env = StatementEnv::default();

        let _output = analyze_statement_sequence(parsed.tree().root_node(), &parsed, &mut env);

        assert_eq!(
            env.let_fact("age").and_then(|fact| fact.kind.clone()),
            Some(Kind::Int)
        );
    }

    #[test]
    fn analyze_statement_sequence_shapes_if_branches_from_local_env() {
        let parsed = parse_source(
            SourceId::new("sequence-test"),
            "LET $outer = 1; IF true { LET $branch = $outer + 1; RETURN $branch; } ELSE { LET $branch = 's'; RETURN $branch; };",
        )
        .expect("valid source parses");
        let mut env = StatementEnv::default();

        let output = analyze_statement_sequence(parsed.tree().root_node(), &parsed, &mut env);
        let if_statement = output
            .statements
            .iter()
            .find(|statement| statement.kind == "if_else")
            .expect("if statement is analyzed");

        assert_eq!(
            if_statement.response_shape,
            Some(ResponseShape::Union {
                variants: vec![
                    ResponseShape::Value { kind: Kind::Int },
                    ResponseShape::Value { kind: Kind::String },
                ],
            })
        );
    }

    #[test]
    fn analyze_statement_sequence_shapes_return_from_prior_env() {
        let parsed = parse_source(
            SourceId::new("sequence-test"),
            "LET $age = 42; RETURN $age;",
        )
        .expect("valid source parses");
        let mut env = StatementEnv::default();

        let output = analyze_statement_sequence(parsed.tree().root_node(), &parsed, &mut env);
        let return_statement = output
            .statements
            .iter()
            .find(|statement| statement.kind == "return")
            .expect("return statement is analyzed");

        assert_eq!(
            return_statement.response_shape,
            Some(ResponseShape::Value { kind: Kind::Int })
        );
    }
}
