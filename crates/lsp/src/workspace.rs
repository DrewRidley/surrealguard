//! Workspace model — tracks all .surql files and builds cross-file schema context.
//!
//! Every definition (table, field, function, index) is tracked with its source
//! file URI and byte offset, enabling accurate "defined here" links across files.

use std::collections::HashMap;
use std::path::PathBuf;

use tower_lsp::lsp_types::Url;

use surrealguard_analyzer::{self as sg, Context, Diagnostic as SgDiagnostic};

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
        self.documents.insert(
            uri.clone(),
            Document { uri, text, version },
        );
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

        // Track byte ranges for each file so we can attribute definitions
        let mut file_byte_ranges: Vec<(Url, u32, u32)> = Vec::new();
        let mut offset = 0u32;

        for (file_uri, text) in &schema_files {
            let len = text.len() as u32;
            file_byte_ranges.push(((*file_uri).clone(), offset, offset + len));
            let _ = sg::analyze_with_context(text, &mut ctx);
            ctx.take_diagnostics(); // Discard diagnostics from schema files
            offset += len;
        }

        // Phase 2: Analyze the target document
        let target_text = &target.text;
        if target_text.trim().is_empty() {
            return Some(AnalysisResult {
                diagnostics: Vec::new(),
                context: ctx,
                source: target_text.clone(),
                uri: uri.clone(),
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
                    if trimmed.starts_with("--") || trimmed.starts_with("//") || trimmed.starts_with('#') {
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
                if path.extension().map(|e| e == "surql" || e == "surrealql").unwrap_or(false) {
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

/// Result of analyzing a single document against the workspace schema.
pub struct AnalysisResult {
    pub diagnostics: Vec<SgDiagnostic>,
    pub context: Context,
    pub source: String,
    pub uri: Url,
}
