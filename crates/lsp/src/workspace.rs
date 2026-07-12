//! Workspace model for the LSP surface.
//!
//! The LSP keeps only document text and delegates analysis to the shared
//! `surrealguard-workspace` facade. It must not depend on the old analyzer.

use std::collections::HashMap;
use std::path::PathBuf;

use tower_lsp::lsp_types::Url;

use surrealguard_diagnostics::Finding;
use surrealguard_syntax::span::{ByteRange, SourceSpan};
use surrealguard_workspace::{analyze_workspace, Workspace as AnalysisWorkspace};

/// A tracked document in the workspace.
#[derive(Debug, Clone)]
pub struct Document {
    pub uri: Url,
    pub text: String,
}

/// The workspace tracks all open/saved documents and provides analysis through
/// the shared workspace facade.
#[derive(Debug, Default)]
pub struct Workspace {
    /// All tracked documents, keyed by URI.
    documents: HashMap<Url, Document>,
    /// Workspace root folders.
    pub roots: Vec<PathBuf>,
}

impl Workspace {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update a document's content on open or change.
    pub fn upsert(&mut self, uri: Url, text: String) {
        self.documents.insert(uri.clone(), Document { uri, text });
    }

    /// Remove a document on close.
    pub fn remove(&mut self, uri: &Url) {
        self.documents.remove(uri);
    }

    /// Get all documents.
    pub fn documents(&self) -> impl Iterator<Item = &Document> {
        self.documents.values()
    }

    /// Analyze a document through the shared `surrealguard-workspace`
    /// pipeline. Plain `.surql` documents analyze as themselves; host
    /// documents (TypeScript, Svelte, ...) analyze their embedded `surql`
    /// templates, with findings re-spanned onto the host file.
    pub fn diagnostic_analysis(&self, uri: &Url) -> Option<DiagnosticAnalysisResult> {
        let target = self.documents.get(uri)?;

        if target.text.trim().is_empty() {
            return Some(DiagnosticAnalysisResult {
                diagnostics: Vec::new(),
                source: target.text.clone(),
                texts: std::collections::BTreeMap::new(),
            });
        }

        let mut analysis_workspace = AnalysisWorkspace::default();
        let mut target_source = None;

        // `.surql` documents are the analysis workspace; host documents
        // never enter it directly (they are not SurrealQL).
        let mut documents: Vec<_> = self
            .documents
            .values()
            .filter(|doc| is_surrealql_uri(&doc.uri))
            .collect();
        documents.sort_by(|left, right| left.uri.as_str().cmp(right.uri.as_str()));

        let mut texts = std::collections::BTreeMap::new();
        for doc in documents {
            let source_id = match doc.uri.to_file_path() {
                Ok(path) => analysis_workspace.add_file_source(path, doc.text.clone()),
                Err(_) => {
                    analysis_workspace.add_virtual_source(doc.uri.to_string(), doc.text.clone())
                }
            };
            texts.insert(source_id.to_string(), (doc.uri.clone(), doc.text.clone()));

            if doc.uri == *uri {
                target_source = Some(source_id);
            }
        }

        // Host target: each embedded query becomes a virtual source
        // analyzed against the `.surql` schema loaded above.
        if !is_surrealql_uri(uri) {
            let embedded =
                surrealguard_embed::extract(uri.path(), &target.text);
            let mut queries = Vec::new();
            for (index, query) in embedded.into_iter().enumerate() {
                let source_id = analysis_workspace.add_virtual_source(
                    format!("embedded://{}#{index}", uri.as_str()),
                    query.text.clone(),
                );
                queries.push((source_id, query));
            }
            texts.insert(
                uri.to_string(),
                (uri.clone(), target.text.clone()),
            );

            let workspace_output = analyze_workspace(&analysis_workspace);
            let host_source =
                surrealguard_syntax::source::SourceId::new(uri.to_string());
            let mut diagnostics = Vec::new();
            for (source_id, query) in &queries {
                let Some(source_output) = workspace_output.sources.get(source_id) else {
                    continue;
                };
                for finding in &source_output.diagnostics {
                    diagnostics.push(respan_to_host(finding, query, &host_source));
                }
            }
            return Some(DiagnosticAnalysisResult {
                diagnostics,
                source: target.text.clone(),
                texts,
            });
        }

        let target_source = target_source?;
        let workspace_output = analyze_workspace(&analysis_workspace);
        let diagnostics = workspace_output
            .sources
            .get(&target_source)
            .map(|source_output| source_output.diagnostics.clone())
            .unwrap_or_default();

        Some(DiagnosticAnalysisResult {
            diagnostics,
            source: target.text.clone(),
            texts,
        })
    }

    /// Scan workspace folders for `.surql` and `.surrealql` files and load them.
    pub fn scan_folders(&mut self) {
        for root in &self.roots.clone() {
            for entry in walkdir::WalkDir::new(root)
                .into_iter()
                .filter_map(|entry| entry.ok())
            {
                let path = entry.path();
                let is_surrealql = path
                    .extension()
                    .map(|extension| extension == "surql" || extension == "surrealql")
                    .unwrap_or(false);

                if !is_surrealql {
                    continue;
                }

                if let Ok(text) = std::fs::read_to_string(path) {
                    if let Ok(uri) = Url::from_file_path(path) {
                        self.upsert(uri, text);
                    }
                }
            }
        }
    }
}

fn is_surrealql_uri(uri: &Url) -> bool {
    let path = uri.path();
    path.ends_with(".surql") || path.ends_with(".surrealql")
}

/// Rebuilds a finding computed on an embedded query so its primary span
/// points into the host file. Related spans (schema declarations) stay
/// where they are.
fn respan_to_host(
    finding: &Finding,
    query: &surrealguard_embed::EmbeddedQuery,
    host_source: &surrealguard_syntax::source::SourceId,
) -> Finding {
    let embedded = finding.span().range();
    let host = query.host_span(embedded.start() as usize..embedded.end() as usize);
    let span = SourceSpan::new(
        host_source.clone(),
        ByteRange::new(host.start as u32, host.end as u32)
            .expect("host spans are ordered"),
    );
    let mut rebuilt = Finding::new(span, finding.code(), finding.severity(), finding.message());
    for help in finding.help() {
        rebuilt = rebuilt.with_help(help.message.clone());
    }
    for related in finding.related() {
        rebuilt = rebuilt.with_related(related.span.clone(), related.message.clone());
    }
    for tag in finding.tags() {
        rebuilt = rebuilt.with_tag(*tag);
    }
    rebuilt
}

/// Diagnostics-only result from the shared workspace analysis facade.
pub struct DiagnosticAnalysisResult {
    pub diagnostics: Vec<Finding>,
    pub source: String,
    /// Every analyzed document keyed by its analysis source id, for
    /// resolving related-information spans that point at other files.
    pub texts: std::collections::BTreeMap<String, (Url, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_analysis_uses_workspace_finding_pipeline_for_target_document() {
        let mut workspace = Workspace::new();
        let target_uri = Url::parse("file:///workspace/query.surql").expect("valid uri");
        workspace.upsert(target_uri.clone(), "SELECT * FROM ;".into());

        let analysis = workspace
            .diagnostic_analysis(&target_uri)
            .expect("target document should be analyzed");

        assert_eq!(analysis.source, "SELECT * FROM ;");
        assert_eq!(analysis.diagnostics.len(), 1);
        assert_eq!(analysis.diagnostics[0].code().to_string(), "S0001");
        assert_eq!(
            analysis.diagnostics[0].span().source().as_str(),
            "file:///workspace/query.surql"
        );
    }
}
