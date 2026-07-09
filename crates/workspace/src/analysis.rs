//! Workspace orchestration: gathers sources, runs schema extraction and
//! per-statement analysis, and assembles the public `AnalysisOutput`
//! (response kinds, inferred params, findings) per source.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use surrealdb_types::Kind;
use surrealguard_diagnostics::{Finding, FindingCode, Severity};
use surrealguard_syntax::parse::{
    parse_source, ParseError, SyntaxDiagnostic, SyntaxDiagnosticKind,
};
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use crate::config::WorkspaceConfig;
use crate::schema::SchemaIndex;
use crate::semantic::{analyze_parsed_source, analyze_sources_in_source_order};
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
    pub response_kind: Option<Kind>,
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
    pub response_kind: Option<Kind>,
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
                response_kind: None,
            }
        }
        Err(error) => AnalysisOutput {
            diagnostics: vec![parse_error_to_finding(source, error)],
            statements: Vec::new(),
            inferred_params: Vec::new(),
            response_kind: None,
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

    let source_ordered = analyze_sources_in_source_order(&parsed_sources);
    for diagnostic in &source_ordered.diagnostics {
        if let Some(source_output) = sources.get_mut(diagnostic.span().source()) {
            source_output.diagnostics.push(diagnostic.clone());
        }
    }
    diagnostics.extend(source_ordered.diagnostics.iter().cloned());

    for inference in source_ordered.param_kind_inferences {
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

    let response_kinds = source_ordered.response_kinds;
    let mut response_kind_counts_by_source = BTreeMap::new();
    for (span, _) in &response_kinds {
        *response_kind_counts_by_source
            .entry(span.source().clone())
            .or_insert(0usize) += 1;
    }
    for (span, kind) in response_kinds {
        if let Some(source_output) = sources.get_mut(span.source()) {
            for statement in &mut source_output.statements {
                if statement.span == span {
                    statement.response_kind = Some(kind.clone());
                }
            }
            if response_kind_counts_by_source[span.source()] == 1
                && source_output.response_kind.is_none()
            {
                source_output.response_kind = Some(kind);
            }
        }
    }

    WorkspaceAnalysis {
        sources,
        diagnostics,
        schema: source_ordered.schema,
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
    use crate::expression::PartialReason;
    use surrealdb_types::Kind;
    use surrealdb_types::KindLiteral;

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
        assert!(output.statements[0].response_kind.is_none());
        assert!(output.inferred_params.is_empty());
        assert!(output.response_kind.is_none());
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
        assert_eq!(diagnostic.severity(), Severity::Error);
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
    fn analyze_workspace_validates_define_index_table_and_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD profile.email ON person TYPE string;\nDEFINE INDEX by_name ON TABLE person FIELDS name, profile.email;".into(),
        );

        let output = analyze_workspace(&workspace);

        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::schema(1002)));
        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::schema(1004)));
    }

    #[test]
    fn analyze_workspace_reports_define_index_unknown_table_and_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE INDEX by_email ON TABLE person FIELDS email;\nDEFINE INDEX missing_table_idx ON TABLE ghost FIELDS name;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| matches!(finding.code(), code if code == FindingCode::schema(1002) || code == FindingCode::schema(1004)))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "index `by_email` references unknown field `email` on table `person`",
                "index `missing_table_idx` targets unknown table `ghost`",
            ]
        );
    }

    #[test]
    fn analyze_workspace_validates_define_event_table_and_when_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE EVENT adult ON TABLE person WHEN $event.age >= 18 THEN { UPDATE person SET age = $event.age; };".into(),
        );

        let output = analyze_workspace(&workspace);

        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::schema(1002)));
        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::schema(1004)));
    }

    #[test]
    fn analyze_workspace_reports_define_event_unknown_table_and_when_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE EVENT bad_field ON TABLE person WHEN $event.missing >= 18 THEN { RETURN true; };\nDEFINE EVENT bad_table ON TABLE ghost WHEN $event.age > 18 THEN { RETURN true; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| matches!(finding.code(), code if code == FindingCode::schema(1002) || code == FindingCode::schema(1004)))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "event `bad_field` references unknown field `missing` on table `person`",
                "event `bad_table` targets unknown table `ghost`",
            ]
        );
    }

    #[test]
    fn analyze_workspace_validates_rebuild_and_remove_index_targets() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE INDEX by_name ON TABLE person FIELDS name;\nREBUILD INDEX by_name ON TABLE person;\nREMOVE INDEX by_name ON TABLE person;\nREBUILD INDEX missing ON TABLE person;\nREMOVE INDEX by_name ON TABLE ghost;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| matches!(finding.code(), code if code == FindingCode::schema(1002) || code == FindingCode::schema(1005)))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "unknown index `missing` on table `person` in REBUILD statement",
                "index `by_name` targets unknown table `ghost` in REMOVE statement",
            ]
        );
    }

    #[test]
    fn analyze_workspace_validates_remove_table_and_field_targets() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD profile.email ON person TYPE string;\nREMOVE FIELD name ON TABLE person;\nREMOVE FIELD profile.email ON person;\nREMOVE TABLE ghost;\nREMOVE FIELD missing ON TABLE person;\nREMOVE FIELD name ON TABLE ghost;\nREMOVE TABLE person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| matches!(finding.code(), code if code == FindingCode::schema(1002) || code == FindingCode::schema(1004)))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "REMOVE TABLE targets unknown table `ghost`",
                "REMOVE FIELD targets unknown field `missing` on table `person`",
                "REMOVE FIELD `name` targets unknown table `ghost`",
            ]
        );
    }

    #[test]
    fn analyze_workspace_validates_alter_table_targets() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nALTER TABLE person DROP;\nALTER TABLE ghost DROP;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1002))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(messages, vec!["ALTER TABLE targets unknown table `ghost`"]);
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
    fn analyze_workspace_resolves_structured_field_types() {
        // Step 6 acceptance: parameterized/union/option types resolve to
        // real kinds instead of degrading to UnsupportedSyntax.
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE FIELD tags ON person TYPE array<string>;\nDEFINE FIELD age ON person TYPE option<int>;\nDEFINE FIELD status ON person TYPE 'active' | 'inactive';".into(),
        );

        let output = analyze_workspace(&workspace);

        let fields = &output.schema.tables["person"].fields;
        assert_eq!(
            fields["tags"].kind,
            Some(Kind::Array(Box::new(Kind::String), None))
        );
        assert_eq!(
            fields["age"].kind,
            Some(Kind::Either(vec![Kind::None, Kind::Int]))
        );
        assert_eq!(
            fields["status"].kind,
            Some(Kind::Either(vec![
                Kind::Literal(KindLiteral::String("active".into())),
                Kind::Literal(KindLiteral::String("inactive".into())),
            ]))
        );
        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::dynamic(6001)));
    }

    #[test]
    fn analyze_workspace_marks_unsupported_field_type_syntax_as_partial_analysis() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE FIELD shape ON person TYPE geometry<point>;".into(),
        );

        let output = analyze_workspace(&workspace);

        let field = &output.schema.tables["person"].fields["shape"];
        assert!(field.kind.is_none());
        assert_eq!(
            field.partial,
            vec![PartialReason::UnsupportedSyntax("geometry<...>".into())]
        );
        let partial: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::dynamic(6001))
            .collect();
        assert_eq!(partial.len(), 1);
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
            .filter(|finding| finding.code() == FindingCode::schema(1001))
            .collect();
        assert_eq!(unknown_tables.len(), 1);
        assert_eq!(unknown_tables[0].message(), "unknown table `company`");
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
    fn analyze_workspace_treats_removed_table_as_absent_for_later_references() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nREMOVE TABLE person;\nUPDATE person SET name = 'Ada';".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1001))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(messages, vec!["unknown table `person`"]);
        assert!(output.schema.table("person").is_none());
    }

    #[test]
    fn analyze_workspace_does_not_use_later_table_definitions_for_earlier_statements() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "UPDATE person SET name = 'Ada';\nDEFINE TABLE person;\nUPDATE person SET name = 'Grace';".into(),
        );

        let output = analyze_workspace(&workspace);
        let unknown_tables: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1001))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(unknown_tables, vec!["unknown table `person`"]);
        assert!(output.schema.table("person").is_some());
    }

    #[test]
    fn analyze_workspace_applies_field_definitions_and_removals_in_source_order() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person SCHEMAFULL;\nUPDATE person SET name = 'Ada';\nDEFINE FIELD name ON person TYPE int;\nUPDATE person SET name = 'Grace';\nREMOVE FIELD name ON person;\nUPDATE person SET name = 'Hedy';".into(),
        );

        let output = analyze_workspace(&workspace);
        let type_mismatches: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::type_error(2001))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            type_mismatches,
            vec!["field `name` expects `int`, found `string`".to_string()]
        );
        assert!(output
            .schema
            .field("person", &crate::schema::FieldPath::parse("name"))
            .is_none());
    }

    #[test]
    fn analyze_workspace_overwrite_replaces_definition_for_downstream_statements() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE user;\nDEFINE TABLE org;\nDEFINE TABLE person TYPE RELATION IN user OUT org;\nRELATE org:acme->person->user:drew;\nDEFINE TABLE OVERWRITE person TYPE RELATION IN org OUT user;\nRELATE org:acme->person->user:drew;".into(),
        );

        let output = analyze_workspace(&workspace);
        let endpoint_messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::graph(3006))
            .map(|finding| finding.message().to_string())
            .collect();

        // The first RELATE (before OVERWRITE) contradicts the declared
        // shape; the second (after) matches the overwritten relation.
        assert_eq!(
            endpoint_messages,
            vec![
                "relation `person` connects `user`->`person`->`org`, but this RELATE writes `org`->`person`->`user`",
            ]
        );
        let relation = output
            .schema
            .table("person")
            .and_then(|table| table.relation.as_ref())
            .unwrap();
        assert_eq!(relation.in_tables, vec!["org".to_string()]);
        assert_eq!(relation.out_tables, vec!["user".to_string()]);
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
            .filter(|finding| finding.code() == FindingCode::schema(1001))
            .map(|finding| finding.message().to_string())
            .collect();
        assert_eq!(
            messages,
            vec![
                "unknown table `ghost`",
                "unknown table `phantom`",
                "unknown table `missing`",
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
            .filter(|diagnostic| diagnostic.code() == FindingCode::schema(1001))
            .map(|diagnostic| diagnostic.message().to_string())
            .collect();

        assert_eq!(
            unknown_tables,
            vec![
                "unknown table `ghost`",
                "unknown table `phantom`",
                "unknown table `missing`",
                "unknown table `shadow`",
                "unknown table `absent`",
                "unknown table `vanished`",
                "unknown table `hidden`",
                "unknown table `obscured`",
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
            source_output.statements[1].response_kind,
            Some(Kind::Any)
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
    fn analyze_workspace_excludes_let_variables_from_query_params() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $age = 42;\nRETURN $age;\nSELECT * FROM person WHERE name = $name;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;

        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "name");
    }

    #[test]
    fn analyze_workspace_infers_return_shape_from_let_variable() {
        let mut workspace = Workspace::default();
        let source =
            workspace.add_virtual_source("query".into(), "LET $age = 42;\nRETURN $age;".into());

        let output = analyze_workspace(&workspace);
        let return_statement = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "return")
            .expect("return statement exists");

        assert_eq!(return_statement.response_kind, Some(Kind::Int));
    }

    #[test]
    fn analyze_workspace_uses_let_variable_kind_for_mutation_assignability() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nLET $age = 42;\nCREATE person SET age = $age;".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];

        assert!(source_output.inferred_params.is_empty());
        assert!(source_output
            .diagnostics
            .iter()
            .all(|finding| finding.code().to_string() != "E2001"));
    }

    #[test]
    fn analyze_workspace_reports_let_variable_kind_mismatch_for_mutation_assignability() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nLET $age = 'old';\nCREATE person SET age = $age;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E2001" && message == "field `age` expects `int`, found `string`"
        }));
    }

    #[test]
    fn analyze_workspace_reports_branch_local_let_mismatch_for_mutation_assignability() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nIF true { LET $age = 'old'; CREATE person SET age = $age; } ELSE { RETURN 0; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E2001" && message == "field `age` expects `int`, found `string`"
        }));
    }

    #[test]
    fn analyze_workspace_infers_let_variables_from_prior_let_variables() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $age = 42;\nLET $next = $age + 1;\nRETURN $next;".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];
        let return_statement = source_output
            .statements
            .iter()
            .find(|statement| statement.kind == "return")
            .expect("return statement exists");

        assert!(source_output.inferred_params.is_empty());
        assert_eq!(return_statement.response_kind, Some(Kind::Int));
    }

    #[test]
    fn analyze_workspace_uses_dependent_let_variable_kind_for_mutation_assignability() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nLET $age = 42;\nLET $next = $age + 1;\nCREATE person SET age = $next;".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];

        assert!(source_output.inferred_params.is_empty());
        assert!(source_output
            .diagnostics
            .iter()
            .all(|finding| finding.code().to_string() != "E2001"));
    }

    #[test]
    fn analyze_workspace_reports_dependent_let_variable_kind_mismatch() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nLET $name = 'Drew';\nLET $excited = $name + '!';\nCREATE person SET age = $excited;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E2001" && message == "field `age` expects `int`, found `string`"
        }));
    }

    #[test]
    fn analyze_workspace_uses_latest_let_shadow_for_return_shape() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $x = 1;\nLET $x = 's';\nRETURN $x;".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];
        let return_statement = source_output
            .statements
            .iter()
            .find(|statement| statement.kind == "return")
            .expect("return statement exists");

        assert!(source_output.inferred_params.is_empty());
        assert_eq!(return_statement.response_kind, Some(Kind::String));
    }

    #[test]
    fn analyze_workspace_preserves_prior_let_dependency_before_shadowing() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $x = 1;\nLET $y = $x + 1;\nLET $x = 's';\nRETURN $y;".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];
        let return_statement = source_output
            .statements
            .iter()
            .find(|statement| statement.kind == "return")
            .expect("return statement exists");

        assert!(source_output.inferred_params.is_empty());
        assert_eq!(return_statement.response_kind, Some(Kind::Int));
    }

    #[test]
    fn analyze_workspace_treats_forward_let_use_as_external_param() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $y = $x + 1;\nLET $x = 1;\nRETURN $y;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;

        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "x");
    }

    #[test]
    fn analyze_workspace_infers_if_else_return_shape_union() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "IF true { RETURN 1; } ELSE { RETURN 's'; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];
        let if_statement = source_output
            .statements
            .iter()
            .find(|statement| statement.kind == "if_else")
            .expect("if/else statement exists");

        assert_eq!(
            if_statement.response_kind,
            Some(Kind::Either(vec![Kind::Int, Kind::String,]))
        );
        assert_eq!(source_output.response_kind, if_statement.response_kind);
    }

    #[test]
    fn analyze_workspace_collapses_matching_if_else_return_shapes() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "IF true { RETURN 1; } ELSE { RETURN 2; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let if_statement = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "if_else")
            .expect("if/else statement exists");

        assert_eq!(if_statement.response_kind, Some(Kind::Int));
    }

    #[test]
    fn analyze_workspace_uses_prior_let_variables_in_if_else_return_shapes() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $age = 42;\nIF true { RETURN $age; } ELSE { RETURN $age + 1; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];
        let if_statement = source_output
            .statements
            .iter()
            .find(|statement| statement.kind == "if_else")
            .expect("if/else statement exists");

        assert!(source_output.inferred_params.is_empty());
        assert_eq!(if_statement.response_kind, Some(Kind::Int));
    }

    #[test]
    fn analyze_workspace_reports_non_bool_if_condition_literals() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "IF 1 { RETURN 1; } ELSE { RETURN 2; };\nIF 'yes' { RETURN 1; } ELSE { RETURN 2; };"
                .into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E2005" && message == "IF condition has type `int`, expected `bool`"
        }));
        assert!(messages.iter().any(|(code, message)| {
            code == "E2005" && message == "IF condition has type `string`, expected `bool`"
        }));
    }

    #[test]
    fn analyze_workspace_allows_bool_and_dynamic_if_conditions() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $flag = true;\nIF $flag { RETURN 1; } ELSE { RETURN 2; };\nIF $runtime { RETURN 1; } ELSE { RETURN 2; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];

        assert!(source_output
            .diagnostics
            .iter()
            .all(|finding| finding.code().to_string() != "E2005"));
        assert_eq!(source_output.inferred_params.len(), 1);
        assert_eq!(source_output.inferred_params[0].name, "runtime");
        assert_eq!(source_output.inferred_params[0].kind, None);
    }

    #[test]
    fn analyze_workspace_reports_non_bool_if_condition_from_let() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $flag = 1;\nIF $flag { RETURN 1; } ELSE { RETURN 2; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E2005" && message == "IF condition has type `int`, expected `bool`"
        }));
    }

    #[test]
    fn analyze_workspace_reports_non_bool_if_condition_from_branch_local_let() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "IF true { LET $flag = 1; IF $flag { RETURN 1; } ELSE { RETURN 2; }; } ELSE { RETURN 3; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E2005" && message == "IF condition has type `int`, expected `bool`"
        }));
    }

    #[test]
    fn analyze_workspace_keeps_if_branch_let_variables_local() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "IF true { LET $branch = 1; } ELSE { LET $branch = 2; };\nRETURN $branch;".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];
        let return_statement = source_output
            .statements
            .iter()
            .find(|statement| statement.kind == "return")
            .expect("return statement exists");

        assert_eq!(source_output.inferred_params.len(), 1);
        assert_eq!(source_output.inferred_params[0].name, "branch");
        assert_eq!(source_output.inferred_params[0].kind, None);
        assert!(matches!(return_statement.response_kind, Some(Kind::Any)));
    }

    #[test]
    fn analyze_workspace_resolves_if_branch_local_let_returns() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "IF true { LET $branch = 1; RETURN $branch; } ELSE { LET $branch = 's'; RETURN $branch; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let if_statement = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "if_else")
            .expect("if statement exists");

        let Some(Kind::Either(variants)) = &if_statement.response_kind else {
            panic!(
                "expected IF branch either kind, got {:?}",
                if_statement.response_kind
            );
        };
        assert_eq!(variants.len(), 2);
        assert!(variants.contains(&Kind::Int));
        assert!(variants.contains(&Kind::String));
    }

    #[test]
    fn analyze_workspace_outer_let_is_visible_inside_if_branch() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $outer = 1;\nIF true { LET $branch = $outer + 1; RETURN $branch; } ELSE { RETURN $outer; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let if_statement = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "if_else")
            .expect("if statement exists");

        assert_eq!(if_statement.response_kind, Some(Kind::Int));
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
    fn analyze_workspace_infers_select_comparison_and_boolean_projection_shapes() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD active ON person TYPE bool;\nSELECT age > 18 AS adult, active = true AS matches_active, true AND active AS visible FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(Kind::Array(element, _)) = &select.response_kind else {
            panic!("expected array kind, got {:?}", select.response_kind);
        };
        let Kind::Literal(KindLiteral::Object(fields)) = element.as_ref() else {
            panic!("expected object literal element, got {element:?}");
        };

        assert_eq!(fields["adult"], Kind::Bool);
        assert_eq!(fields["matches_active"], Kind::Bool);
        assert_eq!(fields["visible"], Kind::Bool);
    }

    #[test]
    fn analyze_workspace_infers_param_kinds_from_let_env_predicates() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD strength ON likes TYPE float;\nLET $min_age = 21;\nLET $edge_strength = 0.5;\nSELECT * FROM person WHERE $min_age <= $age_param;\nSELECT * FROM person->(likes WHERE $edge_strength >= $min_strength)->post;".into(),
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
                ("age_param", Some(Kind::Int)),
                ("min_strength", Some(Kind::Float)),
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
            .filter(|finding| finding.code() == FindingCode::schema(1002))
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
    fn analyze_workspace_reports_unknown_select_order_fields() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT * FROM person ORDER BY missing;".into(),
        );

        let output = analyze_workspace(&workspace);
        let unknown_fields: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1010))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            unknown_fields,
            vec!["unknown field `missing` on table `person`".to_string()]
        );
    }

    #[test]
    fn analyze_workspace_reports_unknown_select_group_and_split_fields() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT name FROM person GROUP BY missing_group SPLIT missing_split;".into(),
        );

        let output = analyze_workspace(&workspace);
        let unknown_fields: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .filter(|finding| matches!(finding.code(), code if code == FindingCode::schema(1009) || code == FindingCode::schema(1010)))
            .map(|finding| finding.message().to_string())
            .collect();
        let mut unknown_fields = unknown_fields;
        unknown_fields.sort();

        assert_eq!(
            unknown_fields,
            vec![
                "unknown field `missing_group` on table `person`".to_string(),
                "unknown field `missing_split` on table `person`".to_string(),
            ]
        );
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
            .filter(|finding| finding.code() == FindingCode::schema(1003))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(messages, vec!["unknown field `missing` on table `person`"]);
    }

    #[test]
    fn analyze_workspace_reports_unknown_mutation_assignment_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nCREATE person SET nickname = 'Ada', name = 'Ada';\nUPDATE person SET handle = 'ada', name = 'Ada';\nUPSERT person SET alias = 'ada', name = 'Ada';".into(),
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
            .filter(|finding| matches!(finding.code().number(), 1002..=1011))
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
            .filter(|finding| matches!(finding.code().number(), 2001..=2003))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "field `age` expects `int`, found `string`",
                "field `age` expects `int`, found `string`",
                "field `age` expects `int`, found `string`",
                "column `age` expects `int`, found `string`",
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
            .filter(|finding| finding.code() == FindingCode::type_error(2002))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "field `profile.email` expects `string`, found `int`",
                "field `created_at` expects `datetime`, found `string`",
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
            .filter(|finding| finding.code() == FindingCode::statement(4004))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "INSERT VALUES has 1 value for 2 columns",
                "INSERT VALUES has 3 values for 2 columns",
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
            .filter(|finding| finding.code() == FindingCode::type_error(2003))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(messages, vec!["column `age` expects `int`, found `string`"]);
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
            .filter(|finding| finding.code() == FindingCode::schema(1003))
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
            .filter(|finding| finding.code() == FindingCode::schema(1002))
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
        let Some(Kind::Array(element, _)) = &select.response_kind else {
            panic!("expected array kind, got {:?}", select.response_kind);
        };
        let Kind::Literal(KindLiteral::Object(fields)) = element.as_ref() else {
            panic!("expected object literal element, got {element:?}");
        };
        let Kind::Literal(KindLiteral::Object(profile_fields)) = &fields["profile"] else {
            panic!(
                "expected nested object literal, got {:?}",
                fields["profile"]
            );
        };
        assert_eq!(profile_fields["name"], Kind::String);
    }

    #[test]
    fn analyze_workspace_infers_row_brace_selector_without_alias_shape() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD profile.name ON person TYPE string;\nDEFINE FIELD profile.age ON person TYPE int;\nDEFINE FIELD profile.secret ON person TYPE string;\nSELECT profile.{name, age} FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(Kind::Array(element, _)) = &select.response_kind else {
            panic!("expected array kind, got {:?}", select.response_kind);
        };
        let Kind::Literal(KindLiteral::Object(fields)) = element.as_ref() else {
            panic!("expected object literal element, got {element:?}");
        };
        let Kind::Literal(KindLiteral::Object(profile_fields)) = &fields["profile"] else {
            panic!(
                "expected nested object literal, got {:?}",
                fields["profile"]
            );
        };

        assert_eq!(
            profile_fields
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["age", "name"]
        );
        assert_eq!(profile_fields["name"], Kind::String);
        assert_eq!(profile_fields["age"], Kind::Int);
    }

    #[test]
    fn analyze_workspace_infers_row_brace_selector_alias_shape() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD profile.name ON person TYPE string;\nDEFINE FIELD profile.age ON person TYPE int;\nSELECT profile.{name, age} AS public_profile FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(Kind::Array(element, _)) = &select.response_kind else {
            panic!("expected array kind, got {:?}", select.response_kind);
        };
        let Kind::Literal(KindLiteral::Object(fields)) = element.as_ref() else {
            panic!("expected object literal element, got {element:?}");
        };
        let Kind::Literal(KindLiteral::Object(profile_fields)) = &fields["public_profile"] else {
            panic!(
                "expected nested object literal, got {:?}",
                fields["public_profile"]
            );
        };

        assert_eq!(profile_fields["name"], Kind::String);
        assert_eq!(profile_fields["age"], Kind::Int);
    }

    #[test]
    fn analyze_workspace_reports_binary_expression_mismatch_from_let_env() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $age = 42;\nLET $bad = $age + 'x';\nRETURN $age + 'x';\nIF true { LET $score = 7; RETURN $score + 'x'; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        let binary_mismatches = messages
            .iter()
            .filter(|(code, message)| {
                code == "E2004" && message == "incompatible operands for `+`: `int` and `string`"
            })
            .count();
        assert_eq!(binary_mismatches, 3);
    }

    #[test]
    fn analyze_workspace_accepts_temporal_and_collection_arithmetic() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE event;\nDEFINE FIELD at ON event TYPE datetime;\nDEFINE FIELD took ON event TYPE duration;\nDEFINE FIELD tags ON event TYPE array<string>;\nSELECT at + 1w AS soon, at - at AS gap, took * 2 AS twice, tags + ['x'] AS more FROM event WHERE at - took < time::now();".into(),
        );

        let output = analyze_workspace(&workspace);
        let operand_findings: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::type_error(2004))
            .collect();

        // Valid SurrealQL temporal/collection arithmetic must not be flagged.
        assert_eq!(operand_findings, Vec::<&Finding>::new());
    }

    #[test]
    fn analyze_workspace_reports_none_assignment_and_bad_negation() {
        // (2014 negation coverage waits on the grammar: prefix `-` only
        // parses on numeric literals today.)
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD retired_at ON person TYPE option<datetime>;\nUPDATE person SET age = NONE, retired_at = NONE;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        // NONE into non-optional int is 2016; into option<datetime> is fine.
        assert!(messages.iter().any(|(code, message)| {
            code == "E2016" && message == "cannot assign NONE to non-optional field `age` (`int`)"
        }));
        assert!(!messages
            .iter()
            .any(|(_, message)| message.contains("retired_at")));
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
            code == "E2004" && message == "incompatible operands for `+`: `int` and `string`"
        }));
    }

    #[test]
    fn analyze_workspace_infers_extended_verified_function_projection_shapes_and_params() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD title ON person TYPE string;\nDEFINE FIELD tags ON person TYPE array;\nSELECT string::lowercase(name) AS lower, string::uppercase(name) AS upper, string::contains(name, 'a') AS has_needle, string::starts_with(name, 'a') AS starts, string::ends_with(name, 'z') AS ends, array::is_empty(tags) AS no_items FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(Kind::Array(element, _)) = &select.response_kind else {
            panic!("expected array kind, got {:?}", select.response_kind);
        };
        let Kind::Literal(KindLiteral::Object(fields)) = element.as_ref() else {
            panic!("expected object literal element, got {element:?}");
        };
        assert_eq!(fields["lower"], Kind::String);
        assert_eq!(fields["upper"], Kind::String);
        assert_eq!(fields["has_needle"], Kind::Bool);
        assert_eq!(fields["starts"], Kind::Bool);
        assert_eq!(fields["ends"], Kind::Bool);
        assert_eq!(fields["no_items"], Kind::Bool);
    }

    #[test]
    fn analyze_workspace_indexes_define_param_defaults_for_later_references() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE PARAM $age VALUE 42;\nSELECT * FROM person WHERE age = $age;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;

        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "age");
        assert_eq!(params[0].kind, Some(Kind::Int));
        assert!(!params[0].required);
    }

    #[test]
    fn analyze_workspace_uses_define_param_default_for_function_diagnostics() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE PARAM $name VALUE 'Ada';\nSELECT string::len($name) AS name_len FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(
            messages.iter().all(|(code, _)| code != "E2004"),
            "defined string param should satisfy string::len: {messages:?}"
        );
    }

    #[test]
    fn analyze_workspace_infers_params_from_extended_verified_function_signatures() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nSELECT string::lowercase($upper), string::contains($haystack, $needle), array::is_empty($items) FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let param_kinds: Vec<_> = output.sources[&source]
            .inferred_params
            .iter()
            .map(|param| (param.name.as_str(), param.kind.clone()))
            .collect();

        assert_eq!(
            param_kinds,
            vec![
                ("haystack", Some(Kind::String)),
                ("items", Some(Kind::Array(Box::new(Kind::Any), None))),
                ("needle", Some(Kind::String)),
                ("upper", Some(Kind::String)),
            ]
        );
    }

    #[test]
    fn analyze_workspace_reports_function_argument_mismatch_from_branch_local_let() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nIF true { LET $age = 42; SELECT string::len($age) FROM person; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E5003" && message == "`string::len` argument 1 expects `string`, found `int`"
        }));
    }

    #[test]
    fn analyze_workspace_reports_function_mismatch_in_mutation_return_expression() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nLET $age = 42;\nUPDATE person SET age = $age RETURN string::len($age) AS bad_len;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E5003" && message == "`string::len` argument 1 expects `string`, found `int`"
        }));
    }

    #[test]
    fn analyze_workspace_reports_function_misuse_in_where_and_set_positions() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nSELECT * FROM person WHERE string::len(age) > 0;\nUPDATE person SET age = math::abs('x');".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        // WHERE conditions and SET values are walked like any other
        // expression position.
        assert!(messages.iter().any(|(code, message)| {
            code == "E5003" && message == "`string::len` argument 1 expects `string`, found `int`"
        }));
        assert!(messages.iter().any(|(code, message)| {
            code == "E5003" && message == "`math::abs` argument 1 expects a number, found `string`"
        }));
    }

    #[test]
    fn analyze_workspace_reports_unknown_custom_functions() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE FUNCTION fn::greet($name: string) -> string { RETURN 'hi'; };\nRETURN fn::greet('a');\nRETURN fn::gret('a');".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E1015" && message == "unknown function `fn::gret`"
        }));
        // The defined function produces no finding.
        assert!(!messages
            .iter()
            .any(|(_, message)| message.contains("fn::greet")));
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
            code == "E5002" && message == "`string::len` expects 1 argument, found 0"
        }));
        assert!(messages.iter().any(|(code, message)| {
            code == "E5003" && message == "`string::len` argument 1 expects `string`, found `int`"
        }));
        assert!(messages.iter().any(|(code, message)| {
            code == "E5003" && message == "`array::len` argument 1 expects an array, found `int`"
        }));
        assert!(messages.iter().any(|(code, message)| {
            code == "E5001" && message == "unknown function `unknown::fn`"
        }));
    }

    #[test]
    fn analyze_workspace_infers_select_value_expression_response_shape() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nSELECT VALUE age + 1 FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(Kind::Array(element, _)) = &select.response_kind else {
            panic!("expected array kind, got {:?}", select.response_kind);
        };
        assert_eq!(element.as_ref(), &Kind::Int);
    }

    #[test]
    fn analyze_workspace_infers_select_value_function_response_shape() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT VALUE string::lowercase(name) FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(Kind::Array(element, _)) = &select.response_kind else {
            panic!("expected array kind, got {:?}", select.response_kind);
        };
        assert_eq!(element.as_ref(), &Kind::String);
    }

    #[test]
    fn analyze_workspace_reports_unknown_mutation_return_fields() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nUPDATE person RETURN nickname, profile.phone;".into(),
        );

        let output = analyze_workspace(&workspace);
        let unknown_fields: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1006))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            unknown_fields,
            vec![
                "unknown field `nickname` on table `person`".to_string(),
                "unknown field `profile.phone` on table `person`".to_string(),
            ]
        );
    }

    #[test]
    fn analyze_workspace_validates_mutation_return_alias_expressions_without_alias_field_false_positive(
    ) {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nUPDATE person RETURN name AS label, missing AS projected_missing;".into(),
        );

        let output = analyze_workspace(&workspace);
        let unknown_fields: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1006))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            unknown_fields,
            vec!["unknown field `missing` on table `person`".to_string()]
        );
    }

    #[test]
    fn analyze_workspace_infers_mutation_return_expression_shape_from_let_env() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nLET $bonus = 1;\nUPDATE person SET age = 42 RETURN $bonus + 1 AS next_bonus;".into(),
        );

        let output = analyze_workspace(&workspace);
        let update = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "update")
            .expect("update statement exists");

        let Some(Kind::Array(element, _)) = &update.response_kind else {
            panic!("expected array kind, got {:?}", update.response_kind);
        };
        let Kind::Literal(KindLiteral::Object(fields)) = element.as_ref() else {
            panic!("expected object literal element, got {element:?}");
        };
        assert_eq!(
            fields.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["next_bonus"]
        );
        assert_eq!(fields["next_bonus"], Kind::Int);
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
            .filter(|finding| matches!(finding.code(), code if code == FindingCode::schema(1007) || code == FindingCode::schema(1008)))
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
    fn analyze_workspace_reports_unknown_later_multi_hop_graph_edge_table() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE comment;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nSELECT * FROM person->likes->post->missing->comment;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1001))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(messages, vec!["unknown table `missing`"]);
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
            .filter(|finding| finding.code() == FindingCode::schema(1001))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec!["unknown table `missing`", "unknown table `ghost`",]
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
            .filter(|finding| finding.code() == FindingCode::schema(1001))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(messages, vec!["unknown table `missing`"]);
    }

    #[test]
    fn analyze_workspace_reports_clause_value_and_lint_findings() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD tags ON person TYPE array<string>;\nLET $lim = 'a';\nSELECT * FROM person LIMIT $lim;\nSELECT * FROM person START -1;\nSELECT * FROM person FETCH age;\nSELECT * FROM person SPLIT age;\nSELECT * FROM person ORDER BY tags;\nLET $auth = 1;\nRETURN [1, 'a'];\nIF true { RETURN 1; };\nRETURN array::map([1], |$v, $i, $extra| $v);\nSELECT type::field('ghost') FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        for (code, message) in [
            ("E2018", "LIMIT expects an integer, found `string`"),
            ("E2024", "START cannot be negative"),
            ("E1023", "FETCH `age` does nothing: `int` holds no records"),
            (
                "E1024",
                "SPLIT `age` expects a collection field, found `int`",
            ),
            (
                "E2017",
                "ORDER BY on `array<string>` orders by structure, not value",
            ),
            (
                "E6007",
                "`$auth` is a protected parameter and cannot be assigned",
            ),
            ("L7003", "array literal mixes kinds: `int`, `string`"),
            ("L7004", "condition is constant"),
            (
                "E5004",
                "`array::map` calls its closure with 2 arguments; `$extra` is never bound",
            ),
            ("E5005", "`ghost` is not a field of table `person`"),
        ] {
            assert!(
                messages.iter().any(|(c, m)| c == code && m == message),
                "missing {code}: {message}\nhave: {messages:#?}"
            );
        }
    }

    #[test]
    fn analyze_workspace_reports_statement_shape_misuse() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nSELECT * FROM ONLY person;\nSELECT * FROM ONLY person LIMIT 1;\nSELECT * FROM ONLY person:one;\nUPDATE ONLY person SET age = 1;\nUPDATE person SET age = 1, age = 2;\nSELECT age, age FROM person;\nLET $y = { LET $inner = 1; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .filter(|finding| matches!(finding.code().number(), 4001..=4020))
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        let expect = [
            (
                "E4003",
                "ONLY on a whole table needs LIMIT 1 (or a record id target)",
            ),
            ("E4003", "ONLY on a whole table needs a record id target"),
            (
                "W4010",
                "`age` is assigned more than once; the last assignment wins",
            ),
            ("W4011", "duplicate projection key `age`"),
            (
                "W4017",
                "block ends with LET, so its value is NONE — return the value instead",
            ),
        ];
        for (code, message) in expect {
            // Rendered test codes carry the category letter; severities are
            // asserted through the severity() accessor below.
            assert!(
                messages
                    .iter()
                    .any(|(c, m)| c.trim_start_matches(char::is_alphabetic)
                        == code.trim_start_matches(char::is_alphabetic)
                        && m == message),
                "missing {code}: {message} in {messages:?}"
            );
        }
        // The guarded forms produce nothing: LIMIT 1 and record ids are
        // exactly the escape hatches.
        assert_eq!(
            messages.iter().filter(|(_, m)| m.contains("ONLY")).count(),
            2
        );
    }

    #[test]
    fn analyze_workspace_type_checks_edge_filter_conditions() {
        // Both edge-filter forms get full expression checking with the
        // edge table as the row.
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD since ON likes TYPE datetime;\nSELECT * FROM person->likes[WHERE since > 5]->post;\nSELECT * FROM person->(likes WHERE since + 1)->post;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        // Comparisons never fail at runtime (values order by kind), so the
        // cross-kind comparison is a lint; the arithmetic is a type error.
        assert!(messages.contains(&(
            "L7005".to_string(),
            "`>` between `datetime` and `int` orders by kind, not value".to_string()
        )));
        assert!(messages.contains(&(
            "E2004".to_string(),
            "incompatible operands for `+`: `datetime` and `int`".to_string()
        )));
    }

    #[test]
    fn analyze_workspace_reports_unreachable_graph_hop_targets() {
        // `likes` goes to `post`; landing on `comment` is 3003.
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE comment;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nSELECT * FROM person->likes->comment;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::graph(3003))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec!["relation `likes` connects `person`->`likes`->`post`, so this hop cannot land on `comment`"]
        );
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
            .filter(|finding| finding.code() == FindingCode::graph(3002))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec!["relation `likes` connects `person`->`likes`->`post`, but this step traverses `->` from `post`"]
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
            .filter(|finding| finding.code() == FindingCode::graph(3006))
            .collect();

        let messages: Vec<_> = mismatches
            .iter()
            .map(|finding| finding.message().to_string())
            .collect();
        // One finding comparing the declared shape against the written one.
        assert_eq!(
            messages,
            vec![
                "relation `likes` connects `person`->`likes`->`post`, but this RELATE writes `post`->`likes`->`person`",
            ]
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
            .filter(|finding| matches!(finding.code(), code if code == FindingCode::schema(1001) || code == FindingCode::graph(3008)))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "unknown table `missing`",
                "unknown table `ghost` in RELATE endpoint",
                "unknown table `likes`",
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
            .filter(|finding| finding.code() == FindingCode::schema(1003))
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
            .filter(|finding| finding.code() == FindingCode::schema(1003))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec!["unknown field `missing_since` on table `likes`"]
        );
    }
}
