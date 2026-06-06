//! Workspace model for the LSP surface.
//!
//! The LSP keeps only document text and delegates analysis to the shared
//! `surrealguard-workspace` facade. It must not depend on the old analyzer.

use std::collections::HashMap;
use std::path::PathBuf;

use tower_lsp::lsp_types::Url;

use surrealguard_diagnostics::Finding;
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

    /// Analyze a document through the shared `surrealguard-workspace` pipeline.
    pub fn diagnostic_analysis(&self, uri: &Url) -> Option<DiagnosticAnalysisResult> {
        let target = self.documents.get(uri)?;

        if target.text.trim().is_empty() {
            return Some(DiagnosticAnalysisResult {
                diagnostics: Vec::new(),
                source: target.text.clone(),
            });
        }

        let mut analysis_workspace = AnalysisWorkspace::default();
        let mut target_source = None;

        let mut documents: Vec<_> = self.documents.values().collect();
        documents.sort_by(|left, right| left.uri.as_str().cmp(right.uri.as_str()));

        for doc in documents {
            let source_id = match doc.uri.to_file_path() {
                Ok(path) => analysis_workspace.add_file_source(path, doc.text.clone()),
                Err(_) => {
                    analysis_workspace.add_virtual_source(doc.uri.to_string(), doc.text.clone())
                }
            };

            if doc.uri == *uri {
                target_source = Some(source_id);
            }
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

/// Diagnostics-only result from the shared workspace analysis facade.
pub struct DiagnosticAnalysisResult {
    pub diagnostics: Vec<Finding>,
    pub source: String,
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
