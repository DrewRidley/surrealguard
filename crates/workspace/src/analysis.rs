use std::collections::BTreeMap;

use crate::response_shape::ResponseShape;
use serde::{Deserialize, Serialize};
use surrealguard_diagnostics::{Finding, FindingCode, Severity};
use surrealguard_syntax::parse::{
    parse_source, ParseError, SyntaxDiagnostic, SyntaxDiagnosticKind,
};
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use crate::config::WorkspaceConfig;
use crate::schema::{extract_schema, SchemaIndex};
use crate::semantic::{
    analyze_parsed_source, infer_non_select_response_shapes, infer_param_kinds,
    infer_select_response_shapes, validate_function_calls, validate_mutation_fields,
    validate_select_graph_references, validate_select_projection_fields, validate_table_references,
};
use crate::source_registry::SourceRegistry;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    config: WorkspaceConfig,
    registry: SourceRegistry,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisOutput {
    pub diagnostics: Vec<Finding>,
    pub statements: Vec<StatementAnalysis>,
    pub inferred_params: Vec<ParamInference>,
    pub response_shape: Option<ResponseShape>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceAnalysis {
    pub sources: BTreeMap<SourceId, AnalysisOutput>,
    pub diagnostics: Vec<Finding>,
    pub schema: SchemaIndex,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatementAnalysis {
    pub span: SourceSpan,
    pub kind: String,
    pub response_shape: Option<ResponseShape>,
    pub select_modifiers: Vec<SelectModifierAnalysis>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectModifierAnalysis {
    pub kind: String,
    pub span: SourceSpan,
    pub row_preserving: bool,
    pub max_len: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamInference {
    pub name: String,
    pub kind: Option<surrealdb_types::Kind>,
    pub required: bool,
    pub spans: Vec<SourceSpan>,
}

impl Workspace {
    pub fn new(config: WorkspaceConfig) -> Self {
        Self {
            config,
            registry: SourceRegistry::default(),
        }
    }

    pub fn config(&self) -> &WorkspaceConfig {
        &self.config
    }

    pub fn registry(&self) -> &SourceRegistry {
        &self.registry
    }

    pub fn registry_mut(&mut self) -> &mut SourceRegistry {
        &mut self.registry
    }

    pub fn add_file_source(&mut self, path: std::path::PathBuf, text: String) -> SourceId {
        self.registry.add_file(path, text)
    }

    pub fn add_virtual_source(&mut self, name: String, text: String) -> SourceId {
        self.registry.add_virtual(name, text)
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new(WorkspaceConfig::default())
    }
}

pub fn analyze_query(workspace: &mut Workspace, query_text: &str) -> AnalysisOutput {
    let source_id = workspace.add_virtual_source("query".into(), query_text.into());
    analyze_source(workspace, source_id)
}

pub fn analyze_source(workspace: &Workspace, source: SourceId) -> AnalysisOutput {
    let Some(text) = workspace.registry.text(&source) else {
        return AnalysisOutput::default();
    };

    match parse_source(source.clone(), text) {
        Ok(parsed) => {
            let syntax_diagnostics: Vec<_> = parsed
                .syntax_diagnostics()
                .iter()
                .map(syntax_diagnostic_to_finding)
                .collect();
            let semantic_output = analyze_parsed_source(&parsed);
            AnalysisOutput {
                diagnostics: syntax_diagnostics,
                statements: semantic_output.statements,
                inferred_params: semantic_output.inferred_params,
                response_shape: None,
            }
        }
        Err(error) => AnalysisOutput {
            diagnostics: vec![parse_error_to_finding(source, error)],
            statements: Vec::new(),
            inferred_params: Vec::new(),
            response_shape: None,
        },
    }
}

pub fn analyze_workspace(workspace: &Workspace) -> WorkspaceAnalysis {
    let mut sources = BTreeMap::new();
    let mut diagnostics = Vec::new();
    let mut parsed_sources = Vec::new();

    for source in workspace.registry.source_ids() {
        let output = analyze_source(workspace, source.clone());
        diagnostics.extend(output.diagnostics.iter().cloned());
        sources.insert(source.clone(), output);

        if let Some(text) = workspace.registry.text(source) {
            if let Ok(parsed) = parse_source(source.clone(), text) {
                parsed_sources.push(parsed);
            }
        }
    }

    let schema_extraction = extract_schema(&parsed_sources);
    for diagnostic in &schema_extraction.diagnostics {
        if let Some(source_output) = sources.get_mut(diagnostic.span().source()) {
            source_output.diagnostics.push(diagnostic.clone());
        }
    }
    diagnostics.extend(schema_extraction.diagnostics.iter().cloned());

    let table_reference_diagnostics =
        validate_table_references(&parsed_sources, &schema_extraction.schema);
    for diagnostic in &table_reference_diagnostics {
        if let Some(source_output) = sources.get_mut(diagnostic.span().source()) {
            source_output.diagnostics.push(diagnostic.clone());
        }
    }
    diagnostics.extend(table_reference_diagnostics);

    let graph_reference_diagnostics =
        validate_select_graph_references(&parsed_sources, &schema_extraction.schema);
    for diagnostic in &graph_reference_diagnostics {
        if let Some(source_output) = sources.get_mut(diagnostic.span().source()) {
            source_output.diagnostics.push(diagnostic.clone());
        }
    }
    diagnostics.extend(graph_reference_diagnostics);

    let select_projection_diagnostics =
        validate_select_projection_fields(&parsed_sources, &schema_extraction.schema);
    for diagnostic in &select_projection_diagnostics {
        if let Some(source_output) = sources.get_mut(diagnostic.span().source()) {
            source_output.diagnostics.push(diagnostic.clone());
        }
    }
    diagnostics.extend(select_projection_diagnostics);

    let mutation_field_diagnostics =
        validate_mutation_fields(&parsed_sources, &schema_extraction.schema);
    for diagnostic in &mutation_field_diagnostics {
        if let Some(source_output) = sources.get_mut(diagnostic.span().source()) {
            source_output.diagnostics.push(diagnostic.clone());
        }
    }
    diagnostics.extend(mutation_field_diagnostics);

    let function_call_diagnostics =
        validate_function_calls(&parsed_sources, &schema_extraction.schema);
    for diagnostic in &function_call_diagnostics {
        if let Some(source_output) = sources.get_mut(diagnostic.span().source()) {
            source_output.diagnostics.push(diagnostic.clone());
        }
    }
    diagnostics.extend(function_call_diagnostics);

    let param_kind_inferences = infer_param_kinds(&parsed_sources, &schema_extraction.schema);
    for inference in param_kind_inferences {
        if let Some(source_output) = sources.get_mut(&inference.source) {
            if let Some(param) = source_output
                .inferred_params
                .iter_mut()
                .find(|param| param.name == inference.name)
            {
                if param.kind.is_none() {
                    param.kind = Some(inference.kind);
                }
            }
        }
    }

    let mut response_shapes =
        infer_select_response_shapes(&parsed_sources, &schema_extraction.schema);
    response_shapes.extend(infer_non_select_response_shapes(
        &parsed_sources,
        &schema_extraction.schema,
    ));
    let mut response_shape_counts_by_source = BTreeMap::new();
    for (span, _) in &response_shapes {
        *response_shape_counts_by_source
            .entry(span.source().clone())
            .or_insert(0usize) += 1;
    }
    for (span, shape) in response_shapes {
        if let Some(source_output) = sources.get_mut(span.source()) {
            for statement in &mut source_output.statements {
                if statement.span == span {
                    statement.response_shape = Some(shape.clone());
                }
            }
            if response_shape_counts_by_source[span.source()] == 1
                && source_output.response_shape.is_none()
            {
                source_output.response_shape = Some(shape);
            }
        }
    }

    WorkspaceAnalysis {
        sources,
        diagnostics,
        schema: schema_extraction.schema,
    }
}

fn syntax_diagnostic_to_finding(diagnostic: &SyntaxDiagnostic) -> Finding {
    let code = match diagnostic.kind() {
        SyntaxDiagnosticKind::ErrorNode => FindingCode::syntax(1),
        SyntaxDiagnosticKind::MissingNode => FindingCode::syntax(2),
    };

    Finding::new(
        diagnostic.span().clone(),
        code,
        Severity::Error,
        diagnostic.message(),
    )
}

fn parse_error_to_finding(source: SourceId, error: ParseError) -> Finding {
    let span = SourceSpan::new(
        source,
        ByteRange::new(0, 0).expect("zero-width byte range is valid"),
    );

    Finding::new(
        span,
        FindingCode::syntax(0),
        Severity::Error,
        error.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::response_shape::PartialReason;
    use surrealdb_types::Kind;

    #[test]
    fn analyze_query_returns_statement_analysis_for_parseable_surrealql() {
        let mut workspace = Workspace::default();

        let output = analyze_query(&mut workspace, "SELECT * FROM person;");

        assert!(output.diagnostics.is_empty());
        assert_eq!(output.statements.len(), 1);
        assert_eq!(output.statements[0].kind, "select");
        assert_eq!(
            output.statements[0].span.source().as_str(),
            "virtual://query#0"
        );
        assert_eq!(output.statements[0].span.range().start(), 0);
        assert_eq!(output.statements[0].span.range().end(), 20);
        assert!(output.statements[0].response_shape.is_none());
        assert!(output.inferred_params.is_empty());
        assert!(output.response_shape.is_none());
    }

    #[test]
    fn analyze_query_emits_statement_analysis_for_all_parseable_statement_kinds() {
        let mut workspace = Workspace::default();
        let query = r#"
BEGIN;
CANCEL;
COMMIT;
INFO FOR DB;
KILL 'abc';
LIVE SELECT * FROM person;
SHOW CHANGES FOR TABLE person;
SLEEP 1s;
USE NS app DB app;
OPTION IMPORT;
BREAK;
CONTINUE;
FOR $item IN [1] { RETURN $item; };
THROW 'bad';
IF true { RETURN 1; };
LET $name = 'Ada';
DELETE person;
CREATE person;
SELECT * FROM person;
RELATE person:one->likes->post:one;
UPDATE person SET name = 'Ada';
REMOVE TABLE person;
UPSERT person:one SET name = 'Ada';
RETURN 1;
ALTER TABLE person SCHEMAFULL;
DEFINE TABLE person;
REBUILD INDEX by_name ON TABLE person;
INSERT INTO person { name: 'Ada' };
"#;

        let output = analyze_query(&mut workspace, query);

        assert!(
            output.diagnostics.is_empty(),
            "expected all statement fixtures to parse cleanly, got {:?}",
            output.diagnostics
        );
        let kinds: Vec<_> = output
            .statements
            .iter()
            .map(|statement| statement.kind.as_str())
            .collect();
        assert_eq!(
            kinds,
            vec![
                "begin",
                "cancel",
                "commit",
                "info_for",
                "kill",
                "live_select",
                "show",
                "sleep",
                "use",
                "option",
                "break",
                "continue",
                "for",
                "throw",
                "if_else",
                "let",
                "delete",
                "create",
                "select",
                "relate",
                "update",
                "remove",
                "upsert",
                "return",
                "alter",
                "define_table",
                "rebuild",
                "insert",
            ]
        );
    }

    #[test]
    fn analyze_query_surfaces_syntax_diagnostics_with_source_spans() {
        let mut workspace = Workspace::default();

        let output = analyze_query(&mut workspace, "SELECT * FROM ;");

        assert_eq!(output.diagnostics.len(), 1);
        let diagnostic = &output.diagnostics[0];
        assert_eq!(diagnostic.code(), FindingCode::syntax(1));
        assert_eq!(diagnostic.default_severity(), Severity::Error);
        assert_eq!(diagnostic.effective_severity(), Severity::Error);
        assert_eq!(diagnostic.span().source().as_str(), "virtual://query#0");
        assert!(!diagnostic.message().is_empty());
    }

    #[test]
    fn analyze_source_uses_existing_registered_source_id() {
        let mut workspace = Workspace::default();
        let source_id =
            workspace.add_virtual_source("schema".into(), "DEFINE TABLE person;".into());

        let output = analyze_source(&workspace, source_id.clone());

        assert_eq!(
            workspace.registry().text(&source_id),
            Some("DEFINE TABLE person;")
        );
        assert!(output.diagnostics.is_empty());
    }

    #[test]
    fn analyze_workspace_returns_outputs_for_all_registered_sources() {
        let mut workspace = Workspace::default();
        let good = workspace.add_virtual_source("schema".into(), "DEFINE TABLE person;".into());
        let bad = workspace.add_virtual_source("query".into(), "SELECT * FROM ;".into());

        let output = analyze_workspace(&workspace);

        assert_eq!(output.sources.len(), 2);
        assert!(output.sources[&good].diagnostics.is_empty());
        assert_eq!(output.sources[&bad].diagnostics.len(), 1);
        assert_eq!(output.diagnostics.len(), 1);
    }

    #[test]
    fn analyze_workspace_indexes_define_table_declarations() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE TABLE company SCHEMAFULL;".into(),
        );

        let output = analyze_workspace(&workspace);

        assert_eq!(output.schema.tables.len(), 2);
        assert_eq!(output.schema.tables["person"].name, "person");
        assert_eq!(output.schema.tables["company"].source, source);
        let span = &output.schema.tables["person"].name_span;
        assert_eq!(span.source(), &source);
        assert_eq!(span.range().start(), 13);
        assert_eq!(span.range().end(), 19);
    }

    #[test]
    fn analyze_workspace_reports_duplicate_table_declarations() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE TABLE person;".into(),
        );

        let output = analyze_workspace(&workspace);

        assert_eq!(output.schema.tables.len(), 1);
        let duplicates: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1001))
            .collect();
        assert_eq!(duplicates.len(), 1);
        assert_eq!(
            duplicates[0].message(),
            "duplicate table definition `person`"
        );
        assert_eq!(duplicates[0].span().range().start(), 34);
        assert_eq!(duplicates[0].span().range().end(), 40);
    }

    #[test]
    fn analyze_workspace_indexes_define_table_relation_metadata() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;".into(),
        );

        let output = analyze_workspace(&workspace);
        let relation = output.schema.tables["likes"]
            .relation
            .as_ref()
            .expect("likes is indexed as relation table");

        assert_eq!(relation.in_tables, vec!["person"]);
        assert_eq!(relation.out_tables, vec!["post"]);
    }

    #[test]
    fn analyze_workspace_indexes_schemafull_field_declarations() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD profile.name ON person TYPE string;"
                .into(),
        );

        let output = analyze_workspace(&workspace);

        let fields = &output.schema.tables["person"].fields;
        assert_eq!(fields.len(), 1);
        let field = &fields["profile.name"];
        assert_eq!(field.path, vec!["profile", "name"]);
        assert_eq!(field.table, "person");
        assert_eq!(field.kind, Some(Kind::String));
        assert_eq!(field.source, source);
        assert_eq!(field.name_span.range().start(), 45);
        assert_eq!(field.name_span.range().end(), 57);
        assert_eq!(field.table_span.range().start(), 61);
        assert_eq!(field.table_span.range().end(), 67);
    }

    #[test]
    fn analyze_workspace_reports_fields_on_unknown_tables() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE FIELD name ON person TYPE string;".into(),
        );

        let output = analyze_workspace(&workspace);

        assert!(output.schema.tables.is_empty());
        let unknown_table: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1002))
            .collect();
        assert_eq!(unknown_table.len(), 1);
        assert_eq!(
            unknown_table[0].message(),
            "field `name` targets unknown table `person`"
        );
        assert_eq!(unknown_table[0].span().range().start(), 21);
        assert_eq!(unknown_table[0].span().range().end(), 27);
    }

    #[test]
    fn analyze_workspace_marks_unsupported_field_type_syntax_as_partial_analysis() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE FIELD tags ON person TYPE array<string>;".into(),
        );

        let output = analyze_workspace(&workspace);

        let field = &output.schema.tables["person"].fields["tags"];
        assert!(field.kind.is_none());
        assert_eq!(
            field.partial,
            vec![PartialReason::UnsupportedSyntax("array<string>".into())]
        );
        let partial: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::dynamic(6001))
            .collect();
        assert_eq!(partial.len(), 1);
        assert_eq!(
            partial[0].message(),
            "unsupported field type syntax `array<string>` for field `tags`"
        );
    }

    #[test]
    fn analyze_workspace_validates_select_table_references_against_schema() {
        let mut workspace = Workspace::default();
        let query = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nSELECT * FROM company;".into(),
        );

        let output = analyze_workspace(&workspace);

        let unknown_tables: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1003))
            .collect();
        assert_eq!(unknown_tables.len(), 1);
        assert_eq!(
            unknown_tables[0].message(),
            "unknown table `company` in SELECT statement"
        );
        assert_eq!(unknown_tables[0].span().source(), &query);
        assert_eq!(unknown_tables[0].span().range().start(), 35);
        assert_eq!(unknown_tables[0].span().range().end(), 42);
        assert_eq!(output.sources[&query].diagnostics.len(), 1);
    }

    #[test]
    fn analyze_workspace_allows_known_basic_table_references() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nSELECT * FROM person;\nCREATE person;\nUPDATE person SET name = 'A';\nDELETE person;".into(),
        );

        let output = analyze_workspace(&workspace);

        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::schema(1003)));
    }

    #[test]
    fn analyze_workspace_validates_create_update_and_delete_table_references() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "CREATE ghost;\nUPDATE phantom SET seen = true;\nDELETE missing;".into(),
        );

        let output = analyze_workspace(&workspace);

        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1003))
            .map(|finding| finding.message().to_string())
            .collect();
        assert_eq!(
            messages,
            vec![
                "unknown table `ghost` in CREATE statement",
                "unknown table `phantom` in UPDATE statement",
                "unknown table `missing` in DELETE statement",
            ]
        );
    }

    #[test]
    fn analyze_workspace_validates_table_references_for_non_select_statement_forms() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "UPSERT ghost SET seen = true;\nINSERT INTO phantom { seen: true };\nLIVE SELECT * FROM missing;\nALTER TABLE shadow SCHEMAFULL;\nREMOVE TABLE stale;\nREBUILD INDEX by_name ON TABLE absent;\nSHOW CHANGES FOR TABLE vanished;\nINFO FOR TABLE hidden;\nINFO FOR TB obscured;".into(),
        );

        let output = analyze_workspace(&workspace);
        let unknown_tables: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code() == FindingCode::schema(1003))
            .map(|diagnostic| diagnostic.message().to_string())
            .collect();

        assert_eq!(
            unknown_tables,
            vec![
                "unknown table `ghost` in UPSERT statement",
                "unknown table `phantom` in INSERT statement",
                "unknown table `missing` in LIVE SELECT statement",
                "unknown table `shadow` in ALTER statement",
                "unknown table `stale` in REMOVE statement",
                "unknown table `absent` in REBUILD statement",
                "unknown table `vanished` in SHOW statement",
                "unknown table `hidden` in INFO FOR statement",
                "unknown table `obscured` in INFO FOR statement",
            ]
        );
    }

    #[test]
    fn analyze_workspace_emits_statement_analysis_for_registered_sources() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nSELECT * FROM person;\nCREATE person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];

        let kinds: Vec<_> = source_output
            .statements
            .iter()
            .map(|statement| statement.kind.as_str())
            .collect();
        assert_eq!(kinds, vec!["define_table", "select", "create"]);
        assert_eq!(source_output.statements[1].span.source(), &source);
        assert_eq!(source_output.statements[1].span.range().start(), 21);
        assert_eq!(source_output.statements[1].span.range().end(), 41);
        assert!(matches!(
            source_output.statements[1].response_shape,
            Some(ResponseShape::Unknown {
                reason: PartialReason::Unresolved
            })
        ));
    }

    #[test]
    fn analyze_workspace_collects_query_parameters_by_name_with_spans() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nSELECT * FROM person WHERE id = $id OR owner = $id AND team = $team;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;

        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, "id");
        assert_eq!(params[0].kind, None);
        assert!(params[0].required);
        assert_eq!(params[0].spans.len(), 2);
        assert_eq!(params[0].spans[0].range().start(), 53);
        assert_eq!(params[0].spans[0].range().end(), 56);
        assert_eq!(params[0].spans[1].range().start(), 68);
        assert_eq!(params[0].spans[1].range().end(), 71);
        assert_eq!(params[1].name, "team");
        assert_eq!(params[1].spans.len(), 1);
        assert_eq!(params[1].spans[0].range().start(), 83);
        assert_eq!(params[1].spans[0].range().end(), 88);
    }

    #[test]
    fn analyze_workspace_infers_param_kinds_from_function_signatures() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nSELECT string::len($name) AS name_len, array::len($tags) AS tag_count, count() AS total FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;

        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, "name");
        assert_eq!(params[0].kind, Some(Kind::String));
        assert_eq!(params[1].name, "tags");
        assert_eq!(params[1].kind, Some(Kind::Array(Box::new(Kind::Any), None)));
    }

    #[test]
    fn analyze_workspace_skips_function_param_inference_for_unknown_and_wrong_arity_calls() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nSELECT unknown::fn($value), string::len($first, $second), count($bad) FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;

        assert_eq!(params.len(), 4);
        assert!(params.iter().all(|param| param.kind.is_none()));
    }

    #[test]
    fn analyze_workspace_infers_param_kind_from_select_where_field_comparison() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT * FROM person WHERE name = $name;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;

        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "name");
        assert_eq!(params[0].kind, Some(Kind::String));
    }

    #[test]
    fn analyze_workspace_infers_param_kinds_from_select_where_comparison_operators() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD name ON person TYPE string;\nSELECT * FROM person WHERE age > $min_age AND $max_age >= age AND name != $excluded_name;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;
        let param_kinds: Vec<_> = params
            .iter()
            .map(|param| (param.name.as_str(), param.kind.clone()))
            .collect();

        assert_eq!(
            param_kinds,
            vec![
                ("excluded_name", Some(Kind::String)),
                ("max_age", Some(Kind::Int)),
                ("min_age", Some(Kind::Int)),
            ]
        );
    }

    #[test]
    fn analyze_workspace_infers_param_kinds_from_graph_local_predicates() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD created_at ON likes TYPE datetime;\nDEFINE FIELD strength ON likes TYPE float;\nSELECT * FROM person->(likes WHERE created_at > $since AND strength >= $min_strength)->post;\nSELECT * FROM person->likes[WHERE created_at <= $before]->post;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;
        let param_kinds: Vec<_> = params
            .iter()
            .map(|param| (param.name.as_str(), param.kind.clone()))
            .collect();

        assert_eq!(
            param_kinds,
            vec![
                ("before", Some(Kind::Datetime)),
                ("min_strength", Some(Kind::Float)),
                ("since", Some(Kind::Datetime)),
            ]
        );
    }

    #[test]
    fn analyze_workspace_skips_statement_and_param_analysis_for_syntax_error_sources() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source("query".into(), "SELECT * FROM ;".into());

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];

        assert_eq!(source_output.diagnostics.len(), 1);
        assert!(source_output.statements.is_empty());
        assert!(source_output.inferred_params.is_empty());
    }

    #[test]
    fn analyze_workspace_allows_known_select_projection_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD profile.email ON person TYPE string;\nSELECT name, profile.email FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);

        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::schema(1004)));
    }

    #[test]
    fn analyze_workspace_reports_unknown_select_projection_fields() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT nickname, profile.phone FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let unknown_fields: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1004))
            .collect();

        assert_eq!(unknown_fields.len(), 2);
        assert_eq!(
            unknown_fields[0].message(),
            "unknown field `nickname` on table `person`"
        );
        assert_eq!(unknown_fields[0].span().source(), &source);
        assert_eq!(unknown_fields[0].span().range().start(), 69);
        assert_eq!(unknown_fields[0].span().range().end(), 77);
        assert_eq!(
            unknown_fields[1].message(),
            "unknown field `profile.phone` on table `person`"
        );
        assert_eq!(unknown_fields[1].span().range().start(), 79);
        assert_eq!(unknown_fields[1].span().range().end(), 92);
        assert_eq!(output.sources[&source].diagnostics.len(), 2);
    }

    #[test]
    fn analyze_workspace_reports_unknown_select_where_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT * FROM person WHERE missing = true AND name = 'Ada';".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1004))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(messages, vec!["unknown field `missing` on table `person`"]);
    }

    #[test]
    fn analyze_workspace_reports_unknown_mutation_assignment_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nCREATE person SET nickname = 'Ada', name = 'Ada';\nUPDATE person SET handle = 'ada', name = 'Ada';\nUPDATE person UNSET stale = true, name = true;\nUPSERT person SET alias = 'ada', name = 'Ada';".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1004))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "unknown field `nickname` on table `person`",
                "unknown field `handle` on table `person`",
                "unknown field `stale` on table `person`",
                "unknown field `alias` on table `person`",
            ]
        );
    }

    #[test]
    fn analyze_workspace_reports_unknown_object_mutation_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD profile.email ON person TYPE string;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD created_at ON likes TYPE datetime;\nCREATE person CONTENT { nickname: 'Ada', profile: { phone: '555' }, name: 'Ada' };\nINSERT INTO person { handle: 'ada', name: 'Ada' };\nINSERT INTO person (alias, name) VALUES ('ada', 'Ada');\nUPDATE person MERGE { stale: true, name: 'Ada' };\nUPSERT person REPLACE { missing: true, name: 'Ada' };\nRELATE person:one->likes->post:one CONTENT { missing_since: time::now(), created_at: time::now() };".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1004))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "unknown field `nickname` on table `person`",
                "unknown field `profile.phone` on table `person`",
                "unknown field `handle` on table `person`",
                "unknown field `alias` on table `person`",
                "unknown field `stale` on table `person`",
                "unknown field `missing` on table `person`",
                "unknown field `missing_since` on table `likes`",
            ]
        );
    }

    #[test]
    fn analyze_workspace_reports_mutation_value_type_mismatches() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD name ON person TYPE string;\nCREATE person SET age = 'old', name = 'Ada';\nUPDATE person MERGE { age: 'old', name: 'Ada' };\nUPSERT person CONTENT { age: 'old', name: 'Ada' };\nINSERT INTO person (age, name) VALUES ('old', 'Ada');".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::type_error(2001))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "value assigned to `age` has type `string`, expected `int`",
                "value assigned to `age` has type `string`, expected `int`",
                "value assigned to `age` has type `string`, expected `int`",
                "value assigned to `age` has type `string`, expected `int`",
            ]
        );
    }

    #[test]
    fn analyze_workspace_skips_type_mismatch_for_dynamic_mutation_params() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nCREATE person SET age = $age;\nUPDATE person MERGE { age: $age };".into(),
        );

        let output = analyze_workspace(&workspace);
        let type_errors: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::type_error(2001))
            .collect();

        assert!(type_errors.is_empty());
    }

    #[test]
    fn analyze_workspace_reports_nested_and_relate_payload_type_mismatches() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD profile.email ON person TYPE string;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD created_at ON likes TYPE datetime;\nUPDATE person MERGE { profile: { email: 10 } };\nRELATE person:one->likes->post:one CONTENT { created_at: 'yesterday' };".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::type_error(2001))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "value assigned to `profile.email` has type `int`, expected `string`",
                "value assigned to `created_at` has type `string`, expected `datetime`",
            ]
        );
    }

    #[test]
    fn analyze_workspace_reports_tuple_insert_arity_mismatches() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD name ON person TYPE string;\nINSERT INTO person (age, name) VALUES (1);\nINSERT INTO person (age, name) VALUES (1, 'Ada', true);".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::type_error(2002))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "INSERT tuple has 1 value for 2 fields",
                "INSERT tuple has 3 values for 2 fields",
            ]
        );
    }

    #[test]
    fn analyze_workspace_checks_tuple_insert_values_across_flattened_rows() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD name ON person TYPE string;\nINSERT INTO person (age, name) VALUES (1, 'Ada'), ('old', 'Grace');".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::type_error(2001))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec!["value assigned to `age` has type `string`, expected `int`"]
        );
    }

    #[test]
    fn analyze_workspace_reports_unknown_mutation_where_fields() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD age ON person TYPE int;\nUPDATE person SET name = 'Ada' WHERE missing > 0 AND age > 18;\nUPSERT person SET name = 'Ada' WHERE ghost = true AND name = 'Ada';\nDELETE person WHERE stale = true AND age < 99;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1004))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "unknown field `missing` on table `person`",
                "unknown field `ghost` on table `person`",
                "unknown field `stale` on table `person`",
            ]
        );
        assert_eq!(output.sources[&source].diagnostics.len(), 3);
    }

    #[test]
    fn analyze_workspace_infers_param_kinds_from_mutation_where_comparisons() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD name ON person TYPE string;\nUPDATE person SET name = 'Ada' WHERE age > $min_age AND $max_age >= age;\nUPSERT person SET name = 'Ada' WHERE name != $excluded_name;\nDELETE person WHERE age <= $delete_before;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;
        let param_kinds: Vec<_> = params
            .iter()
            .map(|param| (param.name.as_str(), param.kind.clone()))
            .collect();

        assert_eq!(
            param_kinds,
            vec![
                ("delete_before", Some(Kind::Int)),
                ("excluded_name", Some(Kind::String)),
                ("max_age", Some(Kind::Int)),
                ("min_age", Some(Kind::Int)),
            ]
        );
    }

    #[test]
    fn analyze_workspace_validates_aliased_select_projection_source_field() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT nickname AS display_name FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let unknown_fields: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1004))
            .collect();

        assert_eq!(unknown_fields.len(), 1);
        assert_eq!(
            unknown_fields[0].message(),
            "unknown field `nickname` on table `person`"
        );
    }

    #[test]
    fn analyze_workspace_skips_wildcard_select_projection_field_validation() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nSELECT * FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);

        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::schema(1004)));
    }

    #[test]
    fn analyze_workspace_infers_select_wildcard_response_shape_from_schema() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD age ON person TYPE int;\nSELECT * FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(ResponseShape::Array {
            element,
            max_len: None,
        }) = &select.response_shape
        else {
            panic!(
                "expected array response shape, got {:?}",
                select.response_shape
            );
        };
        let ResponseShape::Object {
            fields,
            open: false,
        } = element.as_ref()
        else {
            panic!("expected object element, got {element:?}");
        };
        assert_eq!(fields["name"].kind, Some(Kind::String));
        assert_eq!(fields["age"].kind, Some(Kind::Int));
    }

    #[test]
    fn analyze_workspace_infers_select_projection_response_shape() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD profile.email ON person TYPE string;\nSELECT name, profile.email AS email FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(ResponseShape::Array {
            element,
            max_len: None,
        }) = &select.response_shape
        else {
            panic!(
                "expected array response shape, got {:?}",
                select.response_shape
            );
        };
        let ResponseShape::Object {
            fields,
            open: false,
        } = element.as_ref()
        else {
            panic!("expected object element, got {element:?}");
        };
        assert_eq!(
            fields.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["email", "name"]
        );
        assert_eq!(fields["name"].kind, Some(Kind::String));
        assert_eq!(fields["email"].kind, Some(Kind::String));
    }

    #[test]
    fn analyze_workspace_infers_wildcard_response_shape_with_nested_object_fields() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD profile.name ON person TYPE string;\nDEFINE FIELD profile.email ON person TYPE string;\nSELECT * FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(ResponseShape::Array { element, .. }) = &select.response_shape else {
            panic!(
                "expected array response shape, got {:?}",
                select.response_shape
            );
        };
        let ResponseShape::Object {
            fields,
            open: false,
        } = element.as_ref()
        else {
            panic!("expected object element, got {element:?}");
        };
        assert_eq!(
            fields.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["profile"]
        );
        let ResponseShape::Object {
            fields: profile_fields,
            open: false,
        } = &fields["profile"].shape
        else {
            panic!("expected profile object, got {:?}", fields["profile"].shape);
        };
        assert_eq!(
            profile_fields
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["email", "name"]
        );
        assert_eq!(profile_fields["name"].kind, Some(Kind::String));
        assert_eq!(profile_fields["email"].kind, Some(Kind::String));
    }

    #[test]
    fn analyze_workspace_infers_projected_nested_field_response_shape() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD profile.name ON person TYPE string;\nSELECT profile.name FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(ResponseShape::Array { element, .. }) = &select.response_shape else {
            panic!(
                "expected array response shape, got {:?}",
                select.response_shape
            );
        };
        let ResponseShape::Object {
            fields,
            open: false,
        } = element.as_ref()
        else {
            panic!("expected object element, got {element:?}");
        };
        let ResponseShape::Object {
            fields: profile_fields,
            open: false,
        } = &fields["profile"].shape
        else {
            panic!("expected profile object, got {:?}", fields["profile"].shape);
        };
        assert_eq!(
            profile_fields
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["name"]
        );
        assert_eq!(profile_fields["name"].kind, Some(Kind::String));
    }

    #[test]
    fn analyze_workspace_validates_and_shapes_parent_object_paths() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD profile.name ON person TYPE string;\nSELECT profile FROM person WHERE profile = $profile;".into(),
        );

        let output = analyze_workspace(&workspace);
        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::schema(1004)));
        let params = &output.sources[&source].inferred_params;
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "profile");
        assert_eq!(params[0].kind, None);

        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");
        let Some(ResponseShape::Array { element, .. }) = &select.response_shape else {
            panic!(
                "expected array response shape, got {:?}",
                select.response_shape
            );
        };
        let ResponseShape::Object {
            fields,
            open: false,
        } = element.as_ref()
        else {
            panic!("expected object element, got {element:?}");
        };
        let ResponseShape::Object {
            fields: profile_fields,
            open: false,
        } = &fields["profile"].shape
        else {
            panic!("expected profile object, got {:?}", fields["profile"].shape);
        };
        assert_eq!(profile_fields["name"].kind, Some(Kind::String));
    }

    #[test]
    fn analyze_workspace_applies_omit_to_nested_response_shape() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD profile.name ON person TYPE string;\nDEFINE FIELD profile.secret ON person TYPE string;\nSELECT * OMIT profile.secret FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");
        let Some(ResponseShape::Array { element, .. }) = &select.response_shape else {
            panic!(
                "expected array response shape, got {:?}",
                select.response_shape
            );
        };
        let ResponseShape::Object {
            fields,
            open: false,
        } = element.as_ref()
        else {
            panic!("expected object element, got {element:?}");
        };
        let ResponseShape::Object {
            fields: profile_fields,
            open: false,
        } = &fields["profile"].shape
        else {
            panic!("expected profile object, got {:?}", fields["profile"].shape);
        };
        assert_eq!(
            profile_fields
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["name"]
        );
    }

    #[test]
    fn analyze_workspace_infers_select_literal_and_expression_alias_shapes() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nSELECT 1 AS one, true AS active, age + 1 AS next_age FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(ResponseShape::Array { element, .. }) = &select.response_shape else {
            panic!(
                "expected array response shape, got {:?}",
                select.response_shape
            );
        };
        let ResponseShape::Object {
            fields,
            open: false,
        } = element.as_ref()
        else {
            panic!("expected object element, got {element:?}");
        };
        assert_eq!(fields["one"].kind, Some(Kind::Int));
        assert_eq!(fields["active"].kind, Some(Kind::Bool));
        assert_eq!(fields["next_age"].kind, Some(Kind::Int));
        assert!(fields["next_age"].partial.is_empty());
    }

    #[test]
    fn analyze_workspace_infers_binary_expression_projection_shapes() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD score ON person TYPE float;\nDEFINE FIELD name ON person TYPE string;\nSELECT age + 1 AS next_age, score + 1 AS next_score, name + '!' AS excited FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(ResponseShape::Array { element, .. }) = &select.response_shape else {
            panic!(
                "expected array response shape, got {:?}",
                select.response_shape
            );
        };
        let ResponseShape::Object {
            fields,
            open: false,
        } = element.as_ref()
        else {
            panic!("expected object element, got {element:?}");
        };

        assert_eq!(fields["next_age"].kind, Some(Kind::Int));
        assert_eq!(fields["next_score"].kind, Some(Kind::Float));
        assert_eq!(fields["excited"].kind, Some(Kind::String));
    }

    #[test]
    fn analyze_workspace_reports_binary_expression_type_mismatches() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD name ON person TYPE string;\nSELECT age + name AS bad FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];
        let messages: Vec<_> = source_output
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E2005" && message == "operator `+` cannot combine `int` and `string`"
        }));
    }

    #[test]
    fn analyze_workspace_infers_function_projection_shapes() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD tags ON person TYPE array<string>;\nSELECT string::len(name) AS name_len, array::len(tags) AS tag_count, count() AS total FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(ResponseShape::Array { element, .. }) = &select.response_shape else {
            panic!(
                "expected array response shape, got {:?}",
                select.response_shape
            );
        };
        let ResponseShape::Object {
            fields,
            open: false,
        } = element.as_ref()
        else {
            panic!("expected object element, got {element:?}");
        };
        assert_eq!(fields["name_len"].kind, Some(Kind::Int));
        assert_eq!(fields["tag_count"].kind, Some(Kind::Int));
        assert_eq!(fields["total"].kind, Some(Kind::Int));
    }

    #[test]
    fn analyze_workspace_reports_function_arity_and_argument_kind_mismatches() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nSELECT string::len(), string::len(age), array::len(age), unknown::fn(age) FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];
        let messages: Vec<_> = source_output
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E2003" && message == "function `string::len` expects 1 argument, got 0"
        }));
        assert!(messages.iter().any(|(code, message)| {
            code == "E2004"
                && message == "argument 1 to `string::len` has type `int`, expected `string`"
        }));
        assert!(messages.iter().any(|(code, message)| {
            code == "E2004"
                && message == "argument 1 to `array::len` has type `int`, expected `array`"
        }));
        assert!(messages.iter().any(|(code, message)| {
            code == "E2002" && message == "unknown function `unknown::fn`"
        }));
    }

    #[test]
    fn analyze_workspace_infers_select_value_response_shape() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT VALUE name FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(ResponseShape::Array {
            element,
            max_len: None,
        }) = &select.response_shape
        else {
            panic!(
                "expected array response shape, got {:?}",
                select.response_shape
            );
        };
        assert_eq!(
            element.as_ref(),
            &ResponseShape::Value { kind: Kind::String }
        );
    }

    #[test]
    fn analyze_workspace_infers_mutation_row_response_shape_from_schema() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD age ON person TYPE int;\nCREATE person SET name = 'Ada', age = 30 RETURN AFTER;".into(),
        );

        let output = analyze_workspace(&workspace);
        let create = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "create")
            .expect("create statement exists");

        let Some(ResponseShape::Array { element, .. }) = &create.response_shape else {
            panic!(
                "expected array response shape, got {:?}",
                create.response_shape
            );
        };
        let ResponseShape::Object {
            fields,
            open: false,
        } = element.as_ref()
        else {
            panic!("expected object element, got {element:?}");
        };
        assert_eq!(fields["name"].kind, Some(Kind::String));
        assert_eq!(fields["age"].kind, Some(Kind::Int));
    }

    #[test]
    fn analyze_workspace_marks_mutation_return_none_as_empty_array_shape() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nUPDATE person SET name = 'Ada' RETURN NONE;".into(),
        );

        let output = analyze_workspace(&workspace);
        let update = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "update")
            .expect("update statement exists");

        assert_eq!(
            update.response_shape,
            Some(ResponseShape::Array {
                element: Box::new(ResponseShape::Unknown {
                    reason: PartialReason::UnsupportedSyntax("RETURN NONE".into())
                }),
                max_len: Some(0),
            })
        );
    }

    #[test]
    fn analyze_workspace_infers_mutation_return_field_projection_shape() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD profile.email ON person TYPE string;\nUPDATE person SET age = 42 RETURN name, profile.email;".into(),
        );

        let output = analyze_workspace(&workspace);
        let update = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "update")
            .expect("update statement exists");

        let Some(ResponseShape::Array { element, .. }) = &update.response_shape else {
            panic!(
                "expected array response shape, got {:?}",
                update.response_shape
            );
        };
        let ResponseShape::Object {
            fields,
            open: false,
        } = element.as_ref()
        else {
            panic!("expected object element, got {element:?}");
        };
        assert_eq!(
            fields.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["name", "profile"]
        );
        assert_eq!(fields["name"].kind, Some(Kind::String));
        let ResponseShape::Object {
            fields: profile, ..
        } = &fields["profile"].shape
        else {
            panic!("expected profile object, got {:?}", fields["profile"].shape);
        };
        assert_eq!(profile["email"].kind, Some(Kind::String));
    }

    #[test]
    fn analyze_workspace_infers_mutation_return_diff_patch_shape() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nUPDATE person SET name = 'Ada' RETURN DIFF;".into(),
        );

        let output = analyze_workspace(&workspace);
        let update = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "update")
            .expect("update statement exists");

        let Some(ResponseShape::Array { element, .. }) = &update.response_shape else {
            panic!(
                "expected outer array response shape, got {:?}",
                update.response_shape
            );
        };
        let ResponseShape::Array { element: patch, .. } = element.as_ref() else {
            panic!("expected patch array element, got {element:?}");
        };
        let ResponseShape::Object {
            fields,
            open: false,
        } = patch.as_ref()
        else {
            panic!("expected patch object, got {patch:?}");
        };
        assert_eq!(fields["op"].kind, Some(Kind::String));
        assert_eq!(fields["path"].kind, Some(Kind::String));
        assert_eq!(fields["value"].kind, Some(Kind::Any));
    }

    #[test]
    fn analyze_workspace_applies_omit_to_wildcard_response_shape() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD password ON person TYPE string;\nSELECT * OMIT password FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(ResponseShape::Array { element, .. }) = &select.response_shape else {
            panic!(
                "expected array response shape, got {:?}",
                select.response_shape
            );
        };
        let ResponseShape::Object {
            fields,
            open: false,
        } = element.as_ref()
        else {
            panic!("expected object element, got {element:?}");
        };
        assert_eq!(
            fields.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["name"]
        );
    }

    #[test]
    fn analyze_workspace_infers_literal_limit_as_array_max_len() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT name FROM person LIMIT 5;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(ResponseShape::Array {
            element: _,
            max_len: Some(5),
        }) = &select.response_shape
        else {
            panic!(
                "expected array with max_len=5, got {:?}",
                select.response_shape
            );
        };
    }

    #[test]
    fn analyze_workspace_marks_fetched_fields_as_materialized() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD best_friend ON person TYPE record;\nSELECT * FROM person FETCH best_friend;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(ResponseShape::Array { element, .. }) = &select.response_shape else {
            panic!(
                "expected array response shape, got {:?}",
                select.response_shape
            );
        };
        let ResponseShape::Object { fields, .. } = element.as_ref() else {
            panic!("expected object element, got {element:?}");
        };
        assert!(fields["best_friend"].materialized_by_fetch);
    }

    #[test]
    fn analyze_workspace_marks_nested_fetched_fields_as_materialized() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD profile.best_friend ON person TYPE record;\nSELECT * FROM person FETCH profile.best_friend;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(ResponseShape::Array { element, .. }) = &select.response_shape else {
            panic!(
                "expected array response shape, got {:?}",
                select.response_shape
            );
        };
        let ResponseShape::Object { fields, .. } = element.as_ref() else {
            panic!("expected object element, got {element:?}");
        };
        let ResponseShape::Object {
            fields: profile_fields,
            ..
        } = &fields["profile"].shape
        else {
            panic!("expected profile object, got {:?}", fields["profile"].shape);
        };
        assert!(profile_fields["best_friend"].materialized_by_fetch);
    }

    #[test]
    fn analyze_workspace_validates_omit_and_fetch_field_paths() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT * OMIT password FROM person FETCH friend;".into(),
        );

        let output = analyze_workspace(&workspace);
        let unknown_fields: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1004))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            unknown_fields,
            vec![
                "unknown field `password` on table `person`",
                "unknown field `friend` on table `person`",
            ]
        );
    }

    #[test]
    fn analyze_workspace_exposes_row_preserving_select_modifier_facts() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT name FROM person WHERE name = 'Ada' ORDER BY name LIMIT 5 START 2 TIMEOUT 1s PARALLEL;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let modifiers: Vec<_> = select
            .select_modifiers
            .iter()
            .map(|modifier| {
                (
                    modifier.kind.as_str(),
                    modifier.row_preserving,
                    modifier.max_len,
                )
            })
            .collect();

        assert_eq!(
            modifiers,
            vec![
                ("where", true, None),
                ("order", true, None),
                ("limit", true, Some(5)),
                ("start", true, None),
                ("timeout", true, None),
                ("parallel", true, None),
            ]
        );
    }

    #[test]
    fn analyze_workspace_marks_grouped_select_response_shape_partial() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT name FROM person GROUP BY name;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        assert_eq!(
            select.response_shape,
            Some(ResponseShape::Unknown {
                reason: PartialReason::UnsupportedSyntax("GROUP".into())
            })
        );
    }

    #[test]
    fn analyze_workspace_infers_simple_outbound_graph_traversal_shape() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE FIELD title ON post TYPE string;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nSELECT * FROM person->likes->post;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(ResponseShape::Array { element, .. }) = &select.response_shape else {
            panic!(
                "expected array response shape, got {:?}",
                select.response_shape
            );
        };
        let ResponseShape::Object {
            fields,
            open: false,
        } = element.as_ref()
        else {
            panic!("expected object element, got {element:?}");
        };
        assert_eq!(fields["title"].kind, Some(Kind::String));
    }

    #[test]
    fn analyze_workspace_reports_unknown_graph_edge_and_target_tables() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nSELECT * FROM person->missing->post;\nSELECT * FROM person->likes->ghost;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| matches!(finding.code(), code if code == FindingCode::graph(3001) || code == FindingCode::graph(3002)))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "unknown graph edge table `missing`",
                "unknown graph target table `ghost`",
            ]
        );
    }

    #[test]
    fn analyze_workspace_reports_unknown_parenthesized_graph_edge_table() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nSELECT * FROM person->(missing WHERE created_at > $since)->post;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::graph(3001))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(messages, vec!["unknown graph edge table `missing`"]);
    }

    #[test]
    fn analyze_workspace_reports_mismatched_graph_relation_endpoints() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nSELECT * FROM post->likes->person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::graph(3003))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec!["graph traversal `post->likes->person` does not match relation `likes` endpoints"]
        );
    }

    #[test]
    fn analyze_workspace_allows_matching_relate_relation_endpoints() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nRELATE person:one->likes->post:one;".into(),
        );

        let output = analyze_workspace(&workspace);

        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::graph(3003)));
    }

    #[test]
    fn analyze_workspace_reports_mismatched_relate_relation_endpoints() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nRELATE post:one->likes->person:one;".into(),
        );

        let output = analyze_workspace(&workspace);
        let mismatches: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::graph(3003))
            .collect();

        assert_eq!(mismatches.len(), 1);
        assert_eq!(
            mismatches[0].message(),
            "RELATE traversal `post->likes->person` does not match relation `likes` endpoints"
        );
        assert_eq!(mismatches[0].span().source(), &source);
        assert_eq!(output.sources[&source].diagnostics.len(), 1);
    }

    #[test]
    fn analyze_workspace_reports_unknown_relate_edge_and_target_tables() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nRELATE person:one->missing->post:one;\nRELATE person:one->likes->ghost:one;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| matches!(finding.code(), code if code == FindingCode::graph(3001) || code == FindingCode::graph(3002)))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "unknown RELATE edge table `missing`",
                "unknown RELATE edge table `likes`",
                "unknown RELATE target table `ghost`",
            ]
        );
    }

    #[test]
    fn analyze_workspace_validates_parenthesized_graph_where_against_edge_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD created_at ON likes TYPE datetime;\nSELECT * FROM person->(likes WHERE created_at > $since)->post;\nSELECT * FROM person->(likes WHERE missing_since > $since)->post;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1004))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec!["unknown field `missing_since` on table `likes`"]
        );
    }

    #[test]
    fn analyze_workspace_validates_bracketed_graph_filter_against_edge_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD created_at ON likes TYPE datetime;\nSELECT * FROM person->likes[WHERE created_at > $since]->post;\nSELECT * FROM person->likes[WHERE missing_since > $since]->post;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1004))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec!["unknown field `missing_since` on table `likes`"]
        );
    }
}
