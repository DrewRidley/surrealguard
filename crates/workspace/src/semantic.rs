//! The pre-AST validation engine: diagnostic validators (table references,
//! projection/function/assignability/graph checks) and param inference,
//! walking tree-sitter nodes directly.
//!
//! Frozen: no new behavior lands here. Each validator is deleted outright
//! when its statement's invariants are formalized in the analyzer tree
//! with their own finding codes and messages — nothing here is preserved.
//! Type inference has no remaining paths through this module.

use std::collections::BTreeMap;

use surrealguard_diagnostics::{Finding, FindingCode, Severity};
use surrealguard_syntax::parse::ParsedSource;
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use surrealdb_types::Kind;
use tree_sitter::Node;

use crate::analysis::{ParamInference, SelectModifierAnalysis, StatementAnalysis};
use crate::expression::PartialReason;
use crate::expression::{infer_expression_fact, ExpressionFact, ExpressionValueClass};
use crate::schema::{apply_schema_statement_effects, SchemaIndex};
use crate::select_ir::{select_ir_from_statement, FieldPath, SelectModifier};
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceOrderedSemanticOutput {
    pub schema: SchemaIndex,
    pub diagnostics: Vec<Finding>,
    pub param_kind_inferences: Vec<ParamKindInference>,
    pub response_kinds: Vec<(SourceSpan, Kind)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LetVariableFact {
    kind: Option<Kind>,
}

pub fn analyze_parsed_source(parsed: &ParsedSource) -> SemanticOutput {
    if !parsed.syntax_diagnostics().is_empty() {
        return SemanticOutput::default();
    }

    let mut env = StatementEnv::default();
    analyze_statement_sequence(parsed.tree().root_node(), parsed, &mut env)
}

pub fn analyze_sources_in_source_order(
    parsed_sources: &[ParsedSource],
) -> SourceOrderedSemanticOutput {
    let mut output = SourceOrderedSemanticOutput::default();

    for parsed in parsed_sources {
        if !parsed.syntax_diagnostics().is_empty() {
            continue;
        }

        let mut statements = Vec::new();
        collect_statement_nodes(parsed.tree().root_node(), &mut statements);

        let mut select_param_env = StatementEnv::default();
        let mut mutation_param_env = StatementEnv::default();
        let mut select_shape_env = StatementEnv::default();
        let mut mutation_shape_env = StatementEnv::default();
        let mut statement_shape_env = StatementEnv::default();
        let mut analyzer_env = StatementEnv::default();

        for statement in statements {
            // The analyzer tree is the emission spine: each statement is
            // lowered and dispatched against the schema built from the
            // statements before it, with findings landing directly in the
            // output. The node-based collectors below cover only what has
            // not yet been formalized there; each disappears as its codes
            // land.
            {
                let lowered = surrealguard_syntax::lower::lower_statement(statement, parsed.text());
                let mut ctx = crate::analyzer::context::AnalysisContext::scoped(
                    &output.schema,
                    parsed.source_id().clone(),
                    parsed.text(),
                    &mut output.diagnostics,
                    analyzer_env,
                    None,
                );
                crate::analyzer::statement::analyze_lowered_statement(&mut ctx, &lowered);
                analyzer_env = ctx.into_env();
            }

            collect_select_graph_reference_diagnostics(
                statement,
                parsed,
                &output.schema,
                &mut output.diagnostics,
            );
            collect_select_projection_field_diagnostics(
                statement,
                parsed,
                &output.schema,
                &mut output.diagnostics,
            );
            collect_select_param_kind_inferences_with_env(
                statement,
                parsed,
                &output.schema,
                &mut select_param_env,
                &mut output.param_kind_inferences,
            );
            collect_mutation_param_kind_inferences_with_env(
                statement,
                parsed,
                &output.schema,
                &mut mutation_param_env,
                &mut output.param_kind_inferences,
            );
            collect_select_response_shapes_with_env(
                statement,
                parsed,
                &output.schema,
                &mut select_shape_env,
                &mut output.response_kinds,
            );
            collect_mutation_response_shapes_with_env(
                statement,
                parsed,
                &output.schema,
                &mut mutation_shape_env,
                &mut output.response_kinds,
            );

            let statement_output =
                analyze_statement_sequence(statement, parsed, &mut statement_shape_env);
            for analyzed_statement in statement_output.statements {
                if matches!(analyzed_statement.kind.as_str(), "if_else" | "return") {
                    if let Some(kind) = analyzed_statement.response_kind {
                        output.response_kinds.push((analyzed_statement.span, kind));
                    }
                }
            }

            output.diagnostics.extend(apply_schema_statement_effects(
                statement,
                parsed,
                &mut output.schema,
            ));
        }
    }

    output
}

fn collect_statement_nodes<'tree>(node: Node<'tree>, statements: &mut Vec<Node<'tree>>) {
    if node.kind().ends_with("Statement") {
        statements.push(node);
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_statement_nodes(child, statements);
    }
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

pub fn infer_param_kinds(
    parsed_sources: &[ParsedSource],
    schema: &SchemaIndex,
) -> Vec<ParamKindInference> {
    let mut inferences = Vec::new();

    for parsed in parsed_sources {
        if !parsed.syntax_diagnostics().is_empty() {
            continue;
        }

        let mut select_env = StatementEnv::default();
        collect_select_param_kind_inferences_with_env(
            parsed.tree().root_node(),
            parsed,
            schema,
            &mut select_env,
            &mut inferences,
        );
        let mut mutation_env = StatementEnv::default();
        collect_mutation_param_kind_inferences_with_env(
            parsed.tree().root_node(),
            parsed,
            schema,
            &mut mutation_env,
            &mut inferences,
        );
    }

    inferences
}

pub fn infer_select_response_shapes(
    parsed_sources: &[ParsedSource],
    schema: &SchemaIndex,
) -> Vec<(SourceSpan, Kind)> {
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
) -> Vec<(SourceSpan, Kind)> {
    let mut shapes = Vec::new();

    for parsed in parsed_sources {
        if !parsed.syntax_diagnostics().is_empty() {
            continue;
        }
        let mut env = StatementEnv::default();
        let output = analyze_statement_sequence(parsed.tree().root_node(), parsed, &mut env);
        for statement in output.statements {
            if matches!(statement.kind.as_str(), "if_else" | "return") {
                if let Some(shape) = statement.response_kind {
                    shapes.push((statement.span, shape));
                }
            }
        }
        let mut mutation_env = StatementEnv::default();
        collect_mutation_response_shapes_with_env(
            parsed.tree().root_node(),
            parsed,
            schema,
            &mut mutation_env,
            &mut shapes,
        );
    }

    shapes
}

fn collect_mutation_response_shapes_with_env(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    env: &mut StatementEnv,
    shapes: &mut Vec<(SourceSpan, Kind)>,
) {
    match node.kind() {
        "LetStatement" => {
            define_let_from_statement(node, parsed, env);
            return;
        }
        "Block" => {
            let mut child_env = env.fork_child_scope();
            collect_mutation_response_shape_children_with_env(
                node,
                parsed,
                schema,
                &mut child_env,
                shapes,
            );
            return;
        }
        "CreateStatement" | "InsertStatement" | "UpdateStatement" | "UpsertStatement"
        | "DeleteStatement" | "RelateStatement" => {
            let lowered = surrealguard_syntax::lower::lower_statement(node, parsed.text());
            let mut scratch = Vec::new();
            let mut ctx = crate::analyzer::context::AnalysisContext::scoped(
                schema,
                parsed.source_id().clone(),
                parsed.text(),
                &mut scratch,
                env.clone(),
                None,
            );
            let kind = match &lowered.node {
                surrealguard_syntax::ast::Statement::Create(s) => {
                    crate::analyzer::data::create::create_response_kind(s, &mut ctx)
                }
                surrealguard_syntax::ast::Statement::Update(s) => {
                    crate::analyzer::data::update::update_response_kind(s, &mut ctx)
                }
                surrealguard_syntax::ast::Statement::Upsert(s) => {
                    crate::analyzer::data::upsert::upsert_response_kind(s, &mut ctx)
                }
                surrealguard_syntax::ast::Statement::Delete(s) => {
                    crate::analyzer::data::delete::delete_response_kind(s, &mut ctx)
                }
                surrealguard_syntax::ast::Statement::Insert(s) => {
                    crate::analyzer::data::insert::insert_response_kind(s, &mut ctx)
                }
                surrealguard_syntax::ast::Statement::Relate(s) => {
                    crate::analyzer::data::relate::relate_response_kind(s, &mut ctx)
                }
                _ => Kind::Any,
            };
            shapes.push((node_span(node, parsed.source_id().clone()), kind));
            return;
        }
        _ => {}
    }

    collect_mutation_response_shape_children_with_env(node, parsed, schema, env, shapes);
}

fn collect_mutation_response_shape_children_with_env(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    env: &mut StatementEnv,
    shapes: &mut Vec<(SourceSpan, Kind)>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_mutation_response_shapes_with_env(child, parsed, schema, env, shapes);
    }
}

/// Branch result types merge through upstream `Kind::either`, which
/// flattens nested Eithers and dedups. No branches at all is a poison.
fn merge_response_kinds(kinds: Vec<Kind>) -> Kind {
    if kinds.is_empty() {
        return Kind::Any;
    }
    Kind::either(kinds)
}

fn return_response_kind(
    node: Node<'_>,
    parsed: &ParsedSource,
    let_variables: &BTreeMap<String, LetVariableFact>,
) -> Kind {
    let Some(value) = return_value_node(node) else {
        return Kind::Any;
    };
    if value.kind() == "VariableName" {
        let name = param_name(node_text(value, parsed.text()));
        if let Some(variable) = let_variables.get(&name) {
            if let Some(kind) = variable.kind.clone() {
                return kind;
            }
        }
    }

    infer_expression_fact_with_let_variables(value, parsed, None, let_variables)
        .kind
        .unwrap_or(Kind::Any)
}

fn return_value_node(statement: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = statement.walk();
    statement
        .children(&mut cursor)
        .filter(|child| child.is_named() && child.kind() != "Keyword")
        .last()
}

pub(crate) fn find_descendant_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
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
    shapes: &mut Vec<(SourceSpan, Kind)>,
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
            let lowered = surrealguard_syntax::lower::lower_statement(node, parsed.text());
            let kind = match &lowered.node {
                surrealguard_syntax::ast::Statement::Select(stmt) => {
                    let mut scratch = Vec::new();
                    let mut ctx = crate::analyzer::context::AnalysisContext::scoped(
                        schema,
                        parsed.source_id().clone(),
                        parsed.text(),
                        &mut scratch,
                        env.clone(),
                        None,
                    );
                    crate::analyzer::data::select::select_response_kind(stmt, &mut ctx)
                }
                _ => Kind::Any,
            };
            shapes.push((node_span(node, parsed.source_id().clone()), kind));
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
    shapes: &mut Vec<(SourceSpan, Kind)>,
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
            response_kind: None,
            select_modifiers: select_modifier_analysis_for_node(node, parsed),
        });
        output.statements[statement_index].response_kind =
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
) -> Option<Kind> {
    match node.kind() {
        "LetStatement" => {
            if let Some(value_node) = let_value_node(node) {
                collect_expression_params_with_env(value_node, parsed, env, output);
            }
            define_let_from_statement(node, parsed, env);
            None
        }
        "DefineStatement" if is_define_param_statement(node, parsed) => {
            define_param_default_from_statement(node, parsed, env);
            None
        }
        "ReturnStatement" => {
            let let_variables = let_variable_facts_from_env(env);
            let kind = return_response_kind(node, parsed, &let_variables);
            if let Some(value_node) = return_value_node(node) {
                collect_expression_params_with_env(value_node, parsed, env, output);
            }
            Some(kind)
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
) -> Kind {
    let mut branch_kinds = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        analyze_if_else_child(child, parsed, env, output, &mut branch_kinds);
    }
    merge_response_kinds(branch_kinds)
}

fn analyze_if_else_child(
    node: Node<'_>,
    parsed: &ParsedSource,
    env: &mut StatementEnv,
    output: &mut SemanticOutput,
    branch_kinds: &mut Vec<Kind>,
) {
    if node.kind() == "Block" {
        let mut child_env = env.fork_child_scope();
        let mut child_output = SemanticOutput::default();
        analyze_statement_children(node, parsed, &mut child_env, &mut child_output);
        collect_output_response_kinds(&child_output, branch_kinds);
        merge_param_inferences(&mut output.inferred_params, child_output.inferred_params);
        return;
    }

    if node.kind() == "IfElseStatement" {
        let mut child_env = env.fork_child_scope();
        let child_output = analyze_statement_sequence(node, parsed, &mut child_env);
        collect_output_response_kinds(&child_output, branch_kinds);
        merge_param_inferences(&mut output.inferred_params, child_output.inferred_params);
        return;
    }

    if node.kind() == "VariableName" {
        let name = param_name(node_text(node, parsed.text()));
        if env.let_fact(&name).is_none() {
            let span = node_span(node, parsed.source_id().clone());
            env.record_param_use(name.clone(), span.clone());
            record_param_output_from_env(&mut output.inferred_params, env, name, span);
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        analyze_if_else_child(child, parsed, env, output, branch_kinds);
    }
}

fn collect_output_response_kinds(output: &SemanticOutput, kinds: &mut Vec<Kind>) {
    kinds.extend(
        output
            .statements
            .iter()
            .filter_map(|statement| statement.response_kind.clone()),
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
            let fact = infer_expression_fact(
                value_node_or_unknown(node),
                parsed.source_id(),
                parsed.text(),
                None,
            );
            env.define_let(name, fact);
        }
        return;
    }

    if node.kind() == "DefineStatement" && is_define_param_statement(node, parsed) {
        define_param_default_from_statement(node, parsed, env);
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
            record_param_output_from_env(&mut output.inferred_params, env, name, span);
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_expression_params_with_env(child, parsed, env, output);
    }
}

fn record_param_output_from_env(
    target: &mut Vec<ParamInference>,
    env: &StatementEnv,
    name: String,
    span: SourceSpan,
) {
    let default_kind = env
        .param_default_fact(&name)
        .and_then(|fact| fact.kind.clone());
    let required = default_kind.is_none();
    if let Some(existing) = target.iter_mut().find(|existing| existing.name == name) {
        if existing.kind.is_none() {
            existing.kind = default_kind;
        }
        existing.required &= required;
        existing.spans.push(span);
    } else {
        target.push(ParamInference {
            name,
            kind: default_kind,
            required,
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
                },
            )
        })
        .collect()
}

fn is_define_param_statement(node: Node<'_>, parsed: &ParsedSource) -> bool {
    if node.kind() != "DefineStatement" {
        return false;
    }

    let mut keywords = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "Keyword" {
            keywords.push(node_text(child, parsed.text()).to_ascii_uppercase());
        }
    }

    matches!(keywords.first().map(String::as_str), Some("DEFINE"))
        && matches!(keywords.get(1).map(String::as_str), Some("PARAM"))
}

fn define_param_default_from_statement(
    node: Node<'_>,
    parsed: &ParsedSource,
    env: &mut StatementEnv,
) {
    let Some(name_node) = define_param_name_node(node) else {
        return;
    };
    let Some(value_node) = define_param_value_node(node) else {
        return;
    };

    let name = param_name(node_text(name_node, parsed.text()));
    let fact = infer_expression_fact(value_node, parsed.source_id(), parsed.text(), None);
    env.define_param_default(name, fact);
}

fn define_param_name_node(statement: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = statement.walk();
    let found = statement
        .children(&mut cursor)
        .find(|child| child.kind() == "VariableName");
    found
}

fn define_param_value_node(statement: Node<'_>) -> Option<Node<'_>> {
    let name = define_param_name_node(statement)?;
    let mut cursor = statement.walk();
    statement
        .children(&mut cursor)
        .filter(|child| child.is_named() && child.start_byte() > name.end_byte())
        .last()
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
    infer_expression_fact(node, parsed.source_id(), parsed.text(), row_table)
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
            return fact.with_kind(kind);
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

    select_ir_from_statement(node, parsed.source_id(), parsed.text())
        .modifiers
        .into_iter()
        .map(|modifier| match modifier {
            SelectModifier::Where(span) => row_preserving_modifier("where", span, None),
            SelectModifier::Order(span) => row_preserving_modifier("order", span, None),
            SelectModifier::Limit { span, max_len } => {
                row_preserving_modifier("limit", span, max_len)
            }
            SelectModifier::Start(span) => row_preserving_modifier("start", span, None),
            SelectModifier::Timeout(span) => row_preserving_modifier("timeout", span, None),
            SelectModifier::Parallel(span) => row_preserving_modifier("parallel", span, None),
            SelectModifier::Group(span) => non_row_preserving_modifier("group", span),
            SelectModifier::Split(span) => non_row_preserving_modifier("split", span),
            SelectModifier::Explain(span) => non_row_preserving_modifier("explain", span),
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

fn non_row_preserving_modifier(kind: &str, span: SourceSpan) -> SelectModifierAnalysis {
    SelectModifierAnalysis {
        kind: kind.to_string(),
        span,
        row_preserving: false,
        max_len: None,
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
    let ir = select_ir_from_statement(node, parsed.source_id(), parsed.text());
    if ir.graph_lookups.is_empty() {
        return;
    }
    if !ir.graph_lookups.len().is_multiple_of(2) {
        return;
    }

    let Some(mut current_table) = ir.source.as_ref().and_then(|source| source.table.clone()) else {
        return;
    };

    for pair in ir.graph_lookups.chunks_exact(2) {
        let edge_lookup = &pair[0];
        let target_lookup = &pair[1];
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

        if edge_relation.is_some() && target_exists {
            if let Some(next_table) = crate::analyzer::data::select::resolve_graph_step_target_table(
                &current_table,
                edge_lookup,
                target_lookup,
                schema,
            ) {
                current_table = next_table;
            } else {
                diagnostics.push(Finding::new(
                    node_span(node, parsed.source_id().clone()),
                    FindingCode::graph(3003),
                    Severity::Error,
                    format!(
                        "graph traversal `{current_table}->{edge_table}->{target_table}` does not match relation `{edge_table}` endpoints"
                    ),
                ));
                return;
            }
        }
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

/// Inference only: the result kind of a binary expression, independent of
/// whether the operands satisfy the operator's invariants (see
/// [`binary_operands_compatible`] for that).
fn binary_expression_result_kind(operator: &str, left: &Kind, right: &Kind) -> Option<Kind> {
    match operator.to_ascii_uppercase().as_str() {
        "+" if matches!(left, Kind::String) && matches!(right, Kind::String) => Some(Kind::String),
        "+" | "-" | "*" | "/" if is_numeric_kind(left) && is_numeric_kind(right) => {
            if matches!(left, Kind::Decimal) || matches!(right, Kind::Decimal) {
                Some(Kind::Decimal)
            } else if matches!(left, Kind::Float) || matches!(right, Kind::Float) {
                Some(Kind::Float)
            } else {
                Some(Kind::Int)
            }
        }
        // A comparison produces a bool no matter what it compares; mismatched
        // operands violate an invariant, not the result type.
        "=" | "==" | "!=" | "<" | "<=" | ">" | ">=" => Some(Kind::Bool),
        "AND" | "OR" if matches!(left, Kind::Bool) && matches!(right, Kind::Bool) => {
            Some(Kind::Bool)
        }
        "??" if matches!(left, Kind::None | Kind::Null) => Some(right.clone()),
        "??" if matches!(right, Kind::None | Kind::Null) => Some(left.clone()),
        // Coalescing two known kinds yields one of them.
        "??" => Some(Kind::either(vec![left.clone(), right.clone()])),
        _ => None,
    }
}

/// Whether the operand kinds satisfy the operator's invariants — the
/// detection predicate behind the `E2005` mismatch finding. Deliberately
/// stricter than inference: `name > 18` still *infers* bool while being
/// flagged here.
fn is_numeric_kind(kind: &Kind) -> bool {
    matches!(kind, Kind::Int | Kind::Float | Kind::Decimal | Kind::Number)
}

#[derive(Clone, Debug)]
pub(crate) struct FunctionSignature {
    args: Vec<FunctionArgKind>,
    // The remaining consumers of this table (the function-call validator and
    // param inference) only read the argument expectations.
    #[allow(dead_code)]
    pub(crate) return_kind: Kind,
}

#[derive(Clone, Debug)]
enum FunctionArgKind {
    Exact(Kind),
    Array,
}

pub(crate) fn function_signature(name: &str) -> Option<FunctionSignature> {
    match name {
        "string::len" => Some(FunctionSignature {
            args: vec![FunctionArgKind::Exact(Kind::String)],
            return_kind: Kind::Int,
        }),
        "string::lowercase" | "string::uppercase" => Some(FunctionSignature {
            args: vec![FunctionArgKind::Exact(Kind::String)],
            return_kind: Kind::String,
        }),
        "string::contains" | "string::starts_with" | "string::ends_with" => {
            Some(FunctionSignature {
                args: vec![
                    FunctionArgKind::Exact(Kind::String),
                    FunctionArgKind::Exact(Kind::String),
                ],
                return_kind: Kind::Bool,
            })
        }
        "array::len" => Some(FunctionSignature {
            args: vec![FunctionArgKind::Array],
            return_kind: Kind::Int,
        }),
        "array::is_empty" => Some(FunctionSignature {
            args: vec![FunctionArgKind::Array],
            return_kind: Kind::Bool,
        }),
        "count" => Some(FunctionSignature {
            args: Vec::new(),
            return_kind: Kind::Int,
        }),
        _ => None,
    }
}

fn function_call_name(node: Node<'_>, parsed: &ParsedSource) -> Option<String> {
    let name = first_child_kind(node, "FunctionName")?;
    Some(node_text(name, parsed.text()).trim().to_string())
}

pub(crate) fn function_call_args(node: Node<'_>) -> Vec<Node<'_>> {
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

fn collect_select_projection_field_diagnostics(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    diagnostics: &mut Vec<Finding>,
) {
    // Field references now emit from the SELECT analyzer; the graph-local
    // WHERE check remains here until the graph family lands.
    if node.kind() == "SelectStatement" {
        validate_graph_local_where_fields_for_select_statement(node, parsed, schema, diagnostics);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_select_projection_field_diagnostics(child, parsed, schema, diagnostics);
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

pub(crate) fn kind_is_assignable_to(actual: &Kind, expected: &Kind) -> bool {
    if matches!(expected, Kind::Any) || actual == expected {
        return true;
    }
    // A union accepts anything one of its variants accepts (`option<t>` is
    // `none | t`); a union value fits only where every variant fits.
    if let Kind::Either(variants) = expected {
        return variants
            .iter()
            .any(|variant| kind_is_assignable_to(actual, variant));
    }
    if let Kind::Either(variants) = actual {
        return variants
            .iter()
            .all(|variant| kind_is_assignable_to(variant, expected));
    }
    // A literal kind is assignable wherever its base kind is: `'active'` is
    // a string, `{ a: int }` is an object.
    if let Some(base) = literal_base_kind(actual) {
        return kind_is_assignable_to(&base, expected);
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

/// The base kind a `Kind::Literal` value inhabits, if `kind` is one.
pub(crate) fn literal_base_kind(kind: &Kind) -> Option<Kind> {
    use surrealdb_types::KindLiteral;
    let Kind::Literal(literal) = kind else {
        return None;
    };
    let base = match literal {
        KindLiteral::String(_) => Kind::String,
        KindLiteral::Integer(_) => Kind::Int,
        KindLiteral::Float(_) => Kind::Float,
        KindLiteral::Decimal(_) => Kind::Decimal,
        KindLiteral::Duration(_) => Kind::Duration,
        KindLiteral::Bool(_) => Kind::Bool,
        KindLiteral::Array(kinds) => Kind::Array(
            Box::new(Kind::either(kinds.clone())),
            Some(kinds.len() as u64),
        ),
        KindLiteral::Object(_) => Kind::Object,
    };
    Some(base)
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

// Used by the graph-local WHERE check below; retires with the graph
// family (3xxx).
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

pub(crate) fn is_row_context_field_path_node(node: Node<'_>) -> bool {
    matches!(node.kind(), "Ident" | "Path" | "Idiom")
}

pub(crate) fn field_path_from_node(node: Node<'_>, parsed: &ParsedSource) -> FieldPath {
    let text = node_text(node, parsed.text()).trim().to_string();
    FieldPath {
        segments: text.split('.').map(str::to_string).collect(),
        text,
        span: node_span(node, parsed.source_id().clone()),
    }
}

fn collect_select_param_kind_inferences_with_env(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    env: &mut StatementEnv,
    inferences: &mut Vec<ParamKindInference>,
) {
    match node.kind() {
        "LetStatement" => {
            define_let_from_statement(node, parsed, env);
            return;
        }
        "Block" => {
            let mut child_env = env.fork_child_scope();
            collect_select_param_kind_inference_children_with_env(
                node,
                parsed,
                schema,
                &mut child_env,
                inferences,
            );
            return;
        }
        "SelectStatement" => {
            let let_variables = let_variable_facts_from_env(env);
            infer_param_kinds_for_select_statement(
                node,
                parsed,
                schema,
                &let_variables,
                inferences,
            );
        }
        _ => {}
    }

    collect_select_param_kind_inference_children_with_env(node, parsed, schema, env, inferences);
}

fn collect_select_param_kind_inference_children_with_env(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    env: &mut StatementEnv,
    inferences: &mut Vec<ParamKindInference>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_select_param_kind_inferences_with_env(child, parsed, schema, env, inferences);
    }
}

fn infer_param_kinds_for_select_statement(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    let_variables: &BTreeMap<String, LetVariableFact>,
    inferences: &mut Vec<ParamKindInference>,
) {
    let ir = select_ir_from_statement(node, parsed.source_id(), parsed.text());
    collect_graph_local_param_kind_inferences_for_select_statement(
        node,
        parsed,
        schema,
        let_variables,
        inferences,
    );
    collect_function_param_kind_inferences_in_node(node, parsed, inferences);

    let Some(table_name) = crate::analyzer::data::select::resolved_select_table_name(&ir, schema)
    else {
        return;
    };
    let Some(table) = schema.tables.get(table_name.as_str()) else {
        return;
    };

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "WhereClause" {
            collect_param_kind_inferences_from_expression(
                child,
                parsed,
                table,
                let_variables,
                inferences,
            );
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

fn collect_mutation_param_kind_inferences_with_env(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    env: &mut StatementEnv,
    inferences: &mut Vec<ParamKindInference>,
) {
    match node.kind() {
        "LetStatement" => {
            define_let_from_statement(node, parsed, env);
            return;
        }
        "Block" => {
            let mut child_env = env.fork_child_scope();
            collect_mutation_param_kind_inference_children_with_env(
                node,
                parsed,
                schema,
                &mut child_env,
                inferences,
            );
            return;
        }
        "UpdateStatement" | "UpsertStatement" | "DeleteStatement" => {
            let let_variables = let_variable_facts_from_env(env);
            infer_param_kinds_for_mutation_statement(
                node,
                parsed,
                schema,
                &let_variables,
                inferences,
            );
        }
        _ => {}
    }

    collect_mutation_param_kind_inference_children_with_env(node, parsed, schema, env, inferences);
}

fn collect_mutation_param_kind_inference_children_with_env(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    env: &mut StatementEnv,
    inferences: &mut Vec<ParamKindInference>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_mutation_param_kind_inferences_with_env(child, parsed, schema, env, inferences);
    }
}

fn infer_param_kinds_for_mutation_statement(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    let_variables: &BTreeMap<String, LetVariableFact>,
    inferences: &mut Vec<ParamKindInference>,
) {
    let Some(table_name) =
        crate::analyzer::data::mutation::mutation_table_name(node, parsed.text())
    else {
        return;
    };
    let Some(table) = schema.tables.get(&table_name) else {
        return;
    };

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "WhereClause" {
            collect_param_kind_inferences_from_expression(
                child,
                parsed,
                table,
                let_variables,
                inferences,
            );
        }
    }
}

fn collect_graph_local_param_kind_inferences_for_select_statement(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    let_variables: &BTreeMap<String, LetVariableFact>,
    inferences: &mut Vec<ParamKindInference>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_graph_local_param_kind_inferences_in_node(
            child,
            parsed,
            schema,
            let_variables,
            inferences,
        );
    }
}

fn collect_graph_local_param_kind_inferences_in_node(
    node: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    let_variables: &BTreeMap<String, LetVariableFact>,
    inferences: &mut Vec<ParamKindInference>,
) {
    if node.kind() == "Path" {
        collect_graph_local_param_kind_inferences_in_path(
            node,
            parsed,
            schema,
            let_variables,
            inferences,
        );
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_graph_local_param_kind_inferences_in_node(
            child,
            parsed,
            schema,
            let_variables,
            inferences,
        );
    }
}

fn collect_graph_local_param_kind_inferences_in_path(
    path: Node<'_>,
    parsed: &ParsedSource,
    schema: &SchemaIndex,
    let_variables: &BTreeMap<String, LetVariableFact>,
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

        collect_param_kind_inferences_from_where_descendants(
            child,
            parsed,
            edge_table,
            let_variables,
            inferences,
        );
        if let Some(next) = children.get(index + 1).copied() {
            if next.kind() == "Filter" {
                collect_param_kind_inferences_from_where_descendants(
                    next,
                    parsed,
                    edge_table,
                    let_variables,
                    inferences,
                );
            }
        }
    }
}

fn collect_param_kind_inferences_from_where_descendants(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    let_variables: &BTreeMap<String, LetVariableFact>,
    inferences: &mut Vec<ParamKindInference>,
) {
    if node.kind() == "WhereClause" {
        collect_param_kind_inferences_from_expression(
            node,
            parsed,
            table,
            let_variables,
            inferences,
        );
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_param_kind_inferences_from_where_descendants(
            child,
            parsed,
            table,
            let_variables,
            inferences,
        );
    }
}

fn collect_param_kind_inferences_from_expression(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    let_variables: &BTreeMap<String, LetVariableFact>,
    inferences: &mut Vec<ParamKindInference>,
) {
    if node.kind() == "BinaryExpression" {
        if let Some((param_name, kind)) =
            direct_kind_param_comparison(node, parsed, table, let_variables)
        {
            inferences.push(ParamKindInference {
                source: parsed.source_id().clone(),
                name: param_name,
                kind,
            });
        }
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
        collect_param_kind_inferences_from_expression(
            child,
            parsed,
            table,
            let_variables,
            inferences,
        );
    }
}

fn direct_kind_param_comparison(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    let_variables: &BTreeMap<String, LetVariableFact>,
) -> Option<(String, Kind)> {
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
            nearest_kind_inference_operand_node(&children[..operator_index], OperandSide::Left)?;
        let right = nearest_kind_inference_operand_node(
            &children[operator_index + 1..],
            OperandSide::Right,
        )?;
        let left = kind_or_param_operand(left, parsed, table, let_variables);
        let right = kind_or_param_operand(right, parsed, table, let_variables);
        match (left, right) {
            (Some(KindOrParamOperand::KnownKind(kind)), Some(KindOrParamOperand::Param(param)))
            | (Some(KindOrParamOperand::Param(param)), Some(KindOrParamOperand::KnownKind(kind))) =>
            {
                return Some((param, kind));
            }
            _ => {}
        }
    }

    None
}

#[derive(Clone, Debug)]
enum KindOrParamOperand {
    KnownKind(Kind),
    Param(String),
}

fn nearest_kind_inference_operand_node<'tree>(
    nodes: &[Node<'tree>],
    side: OperandSide,
) -> Option<Node<'tree>> {
    let ordered_nodes: Box<dyn Iterator<Item = Node<'_>> + '_> = match side {
        OperandSide::Left => Box::new(nodes.iter().rev().copied()),
        OperandSide::Right => Box::new(nodes.iter().copied()),
    };

    for node in ordered_nodes {
        if matches!(node.kind(), "Keyword" | "Operator") {
            continue;
        }
        return Some(node);
    }
    None
}

fn kind_or_param_operand(
    node: Node<'_>,
    parsed: &ParsedSource,
    table: &crate::schema::TableDef,
    let_variables: &BTreeMap<String, LetVariableFact>,
) -> Option<KindOrParamOperand> {
    if node.kind() == "VariableName" {
        let name = param_name(node_text(node, parsed.text()));
        if let Some(kind) = let_variables
            .get(&name)
            .and_then(|variable| variable.kind.clone())
        {
            return Some(KindOrParamOperand::KnownKind(kind));
        }
        return Some(KindOrParamOperand::Param(name));
    }

    if is_row_context_field_path_node(node) {
        let field = field_path_from_node(node, parsed);
        return exact_field_def_for_path(table, &field)
            .and_then(|field| field.kind.clone())
            .map(KindOrParamOperand::KnownKind);
    }

    infer_expression_fact_with_let_variables(node, parsed, Some(table), let_variables)
        .kind
        .map(KindOrParamOperand::KnownKind)
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

#[derive(Clone, Copy)]
pub(crate) struct TableReference<'tree> {
    pub(crate) name: &'tree str,
}

pub(crate) fn leading_table_references<'tree>(
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
            });
            continue;
        }

        if is_data_or_modifier_clause(child) {
            break;
        }
    }

    references
}

pub(crate) fn table_references_after_keyword<'tree>(
    node: Node<'tree>,
    source: &'tree str,
    keyword: &str,
) -> Vec<TableReference<'tree>> {
    fn visit<'tree>(
        node: Node<'tree>,
        source: &'tree str,
        keyword: &str,
        saw_keyword: &mut bool,
        references: &mut Vec<TableReference<'tree>>,
    ) {
        let text = node_text(node, source);
        if node.kind() == "Keyword" {
            *saw_keyword = text.eq_ignore_ascii_case(keyword);
        } else if *saw_keyword && is_identifier_like(node) {
            references.push(TableReference {
                name: table_name_from_node_text(text),
            });
            *saw_keyword = false;
        } else if *saw_keyword && (!node.is_named() || is_data_or_modifier_clause(node)) {
            *saw_keyword = false;
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            visit(child, source, keyword, saw_keyword, references);
        }
    }

    let mut saw_keyword = false;
    let mut references = Vec::new();
    visit(node, source, keyword, &mut saw_keyword, &mut references);
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
            if_statement.response_kind,
            Some(Kind::Either(vec![Kind::Int, Kind::String]))
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

        assert_eq!(return_statement.response_kind, Some(Kind::Int));
    }
}
