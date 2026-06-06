//! Workspace model — tracks all .surql files and builds cross-file schema context.
//!
//! Every definition (table, field, function, index) is tracked with its source
//! file URI and byte offset, enabling accurate "defined here" links across files.

use std::collections::HashMap;
use std::path::PathBuf;

use tower_lsp::lsp_types::Url;

use surrealguard_analyzer::{self as sg, Context, Diagnostic as SgDiagnostic};
use surrealguard_diagnostics::Finding;
use surrealguard_workspace::{analyze_workspace, Workspace as AnalysisWorkspace};

/// A tracked document in the workspace.
#[derive(Debug, Clone)]
pub struct Document {
    pub uri: Url,
    pub text: String,
    pub version: i32,
}

/// A definition's source location across files.
#[derive(Debug, Clone)]
pub struct DefinitionLocation {
    pub uri: Url,
    pub start: u32,
    pub end: u32,
}

/// The workspace tracks all open/saved documents and provides
/// cross-file analysis capabilities.
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

    /// Update a document's content (on open or change).
    pub fn upsert(&mut self, uri: Url, text: String, version: i32) {
        self.documents
            .insert(uri.clone(), Document { uri, text, version });
    }

    /// Remove a document (on close).
    pub fn remove(&mut self, uri: &Url) {
        self.documents.remove(uri);
    }

    /// Get a document by URI.
    pub fn get(&self, uri: &Url) -> Option<&Document> {
        self.documents.get(uri)
    }

    /// Get all documents.
    pub fn documents(&self) -> impl Iterator<Item = &Document> {
        self.documents.values()
    }

    /// Build a surrealguard Context with schema from ALL workspace files,
    /// then analyze a specific document against that context.
    ///
    /// Returns diagnostics for the target document only.
    /// Also returns the context (for hover/hints to use).
    pub fn analyze_document(&self, uri: &Url) -> Option<AnalysisResult> {
        let target = self.documents.get(uri)?;

        let mut ctx = Context::new();

        // Phase 1: Load schema from all OTHER files (not the target)
        let mut schema_files: Vec<(&Url, &str)> = Vec::new();
        for doc in self.documents.values() {
            if doc.uri != *uri && !doc.text.trim().is_empty() {
                schema_files.push((&doc.uri, &doc.text));
            }
        }

        let mut schema_sources = Vec::new();

        for (file_uri, text) in &schema_files {
            schema_sources.push(SchemaSource {
                uri: (*file_uri).clone(),
                text: text.to_string(),
            });
            let _ = sg::analyze_with_context(text, &mut ctx);
            ctx.take_diagnostics();
        }

        // Phase 2: Analyze the target document
        let target_text = &target.text;
        if target_text.trim().is_empty() {
            return Some(AnalysisResult {
                diagnostics: Vec::new(),
                context: ctx,
                source: target_text.clone(),
                uri: uri.clone(),
                schema_sources,
            });
        }

        let diagnostics = match sg::analyze_with_context(target_text, &mut ctx) {
            Ok(diags) => diags,
            Err(_) => Vec::new(),
        };

        // Filter diagnostics: only keep those with spans in the target document
        let target_len = target_text.len() as u32;
        let valid_diagnostics: Vec<SgDiagnostic> = diagnostics
            .into_iter()
            .filter(|d| d.span.end <= target_len)
            .map(|mut d| {
                // Filter related spans: only keep those that reference text
                // actually present in the target document
                d.related.retain(|rel| {
                    let start = rel.span.start as usize;
                    let end = rel.span.end as usize;
                    if start >= target_text.len() || end > target_text.len() || start > end {
                        return false;
                    }
                    // Validate: the span text should contain the identifier
                    // mentioned in the message (between first backticks)
                    let span_text = &target_text[start..end];
                    // Reject spans that point into comments
                    let trimmed = span_text.trim_start();
                    if trimmed.starts_with("--")
                        || trimmed.starts_with("//")
                        || trimmed.starts_with('#')
                    {
                        return false;
                    }
                    // Validate: span text should contain the referenced identifier
                    // AND should look like actual SurrealQL (starts with DEFINE, keyword, etc.)
                    if let Some(bt_start) = rel.message.find('`') {
                        if let Some(bt_end) = rel.message[bt_start + 1..].find('`') {
                            let ident = &rel.message[bt_start + 1..bt_start + 1 + bt_end];
                            return span_text.contains(ident)
                                && (span_text.contains("DEFINE")
                                    || span_text.contains("TABLE")
                                    || span_text.contains("FIELD")
                                    || span_text.contains("TYPE")
                                    || span_text.contains("RELATION")
                                    || span_text.len() < 80); // Short spans are likely identifiers
                        }
                    }
                    false
                });
                d
            })
            .collect();

        Some(AnalysisResult {
            diagnostics: valid_diagnostics,
            context: ctx,
            source: target_text.clone(),
            uri: uri.clone(),
            schema_sources,
        })
    }

    /// Analyze a document through the shared surrealguard-workspace pipeline.
    ///
    /// This is diagnostics-only for now: hover, completions, definitions, and
    /// inlay hints still consume the legacy analyzer context until those APIs
    /// are migrated onto the shared facade.
    pub fn diagnostic_analysis(&self, uri: &Url) -> Option<DiagnosticAnalysisResult> {
        let target = self.documents.get(uri)?;

        if target.text.trim().is_empty() {
            return Some(DiagnosticAnalysisResult {
                diagnostics: Vec::new(),
                source: target.text.clone(),
                uri: uri.clone(),
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
            uri: uri.clone(),
        })
    }

    /// Scan workspace folders for .surql files and load them.
    pub fn scan_folders(&mut self) {
        for root in &self.roots.clone() {
            for entry in walkdir::WalkDir::new(root)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                let path = entry.path();
                if path
                    .extension()
                    .map(|e| e == "surql" || e == "surrealql")
                    .unwrap_or(false)
                {
                    if let Ok(text) = std::fs::read_to_string(path) {
                        if let Ok(uri) = Url::from_file_path(path) {
                            self.upsert(uri, text, 0);
                        }
                    }
                }
            }
        }
    }
}

/// A schema file's URI and source text, for cross-file definition lookup.
#[derive(Debug, Clone)]
pub struct SchemaSource {
    pub uri: Url,
    pub text: String,
}

/// Result of analyzing a single document against the workspace schema.
pub struct AnalysisResult {
    pub diagnostics: Vec<SgDiagnostic>,
    pub context: Context,
    pub source: String,
    pub uri: Url,
    /// Schema files that were loaded into context (for cross-file go-to-def).
    pub schema_sources: Vec<SchemaSource>,
}

/// Diagnostics-only result from the shared workspace analysis facade.
pub struct DiagnosticAnalysisResult {
    pub diagnostics: Vec<Finding>,
    pub source: String,
    pub uri: Url,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_analysis_uses_workspace_finding_pipeline_for_target_document() {
        let mut workspace = Workspace::new();
        let target_uri = Url::parse("file:///workspace/query.surql").expect("valid uri");
        workspace.upsert(target_uri.clone(), "SELECT * FROM ;".into(), 1);

        let analysis = workspace
            .diagnostic_analysis(&target_uri)
            .expect("target document should be analyzed");

        assert_eq!(analysis.uri, target_uri);
        assert_eq!(analysis.source, "SELECT * FROM ;");
        assert_eq!(analysis.diagnostics.len(), 1);
        assert_eq!(analysis.diagnostics[0].code().to_string(), "S0001");
        assert_eq!(
            analysis.diagnostics[0].span().source().as_str(),
            "file:///workspace/query.surql"
        );
    }
}
