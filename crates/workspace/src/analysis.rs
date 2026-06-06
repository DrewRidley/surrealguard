use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use surrealguard_diagnostics::{Finding, FindingCode, Severity};
use surrealguard_syntax::parse::{
    parse_source, ParseError, SyntaxDiagnostic, SyntaxDiagnosticKind,
};
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};
use surrealguard_types::Type;

use crate::config::WorkspaceConfig;
use crate::schema::{extract_schema, SchemaIndex};
use crate::semantic::validate_table_references;
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
    pub result_type: Option<Type>,
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
    pub result_type: Option<Type>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamInference {
    pub name: String,
    pub ty: Type,
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
        Ok(parsed) => AnalysisOutput {
            diagnostics: parsed
                .syntax_diagnostics()
                .iter()
                .map(syntax_diagnostic_to_finding)
                .collect(),
            statements: Vec::new(),
            inferred_params: Vec::new(),
            result_type: None,
        },
        Err(error) => AnalysisOutput {
            diagnostics: vec![parse_error_to_finding(source, error)],
            statements: Vec::new(),
            inferred_params: Vec::new(),
            result_type: None,
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

    #[test]
    fn analyze_query_returns_no_diagnostics_for_parseable_surrealql() {
        let mut workspace = Workspace::default();

        let output = analyze_query(&mut workspace, "SELECT * FROM person;");

        assert!(output.diagnostics.is_empty());
        assert!(output.statements.is_empty());
        assert!(output.inferred_params.is_empty());
        assert!(output.result_type.is_none());
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
        assert_eq!(field.ty, Type::String);
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
        assert!(matches!(field.ty, Type::Unknown(_)));
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
}
