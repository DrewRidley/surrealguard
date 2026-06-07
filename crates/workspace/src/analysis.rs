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
    analyze_parsed_source, infer_select_response_shapes, validate_select_projection_fields,
    validate_table_references,
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

    let select_projection_diagnostics =
        validate_select_projection_fields(&parsed_sources, &schema_extraction.schema);
    for diagnostic in &select_projection_diagnostics {
        if let Some(source_output) = sources.get_mut(diagnostic.span().source()) {
            source_output.diagnostics.push(diagnostic.clone());
        }
    }
    diagnostics.extend(select_projection_diagnostics);

    let select_response_shapes =
        infer_select_response_shapes(&parsed_sources, &schema_extraction.schema);
    for (span, shape) in select_response_shapes {
        if let Some(source_output) = sources.get_mut(span.source()) {
            for statement in &mut source_output.statements {
                if statement.span == span {
                    statement.response_shape = Some(shape.clone());
                }
            }
            if source_output.response_shape.is_none() {
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
}
