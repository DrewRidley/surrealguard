//! Workspace model for the LSP surface.
//!
//! The LSP keeps only document text and delegates analysis to the shared
//! `surrealguard-workspace` facade. It must not depend on the old analyzer.
//!
//! # Analysis caching
//!
//! Analyzing the whole `.surql` document set is expensive (a full
//! [`analyze_workspace`] pass over every schema and query file). A single
//! keystroke-then-hover used to trigger two or three of those passes:
//! `did_change` publishes diagnostics for every document, and each hover /
//! inlay / go-to-definition re-ran the analysis for the target file. To keep
//! the editor reactive we cache the whole-workspace analysis keyed by a hash
//! of the analysis inputs (every `.surql` document's URI and text). Within a
//! single unchanged document state the pass runs at most once; every
//! subsequent request reuses the cached [`WorkspaceAnalysis`]. Any edit
//! (`upsert`) or close (`remove`) changes the key, so the next request
//! recomputes — the cache can never serve analysis that predates the latest
//! edit.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use tower_lsp::lsp_types::Url;

use surrealguard_diagnostics::Finding;
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};
use surrealguard_workspace::{
    analyze_workspace, AnalysisOutput, SchemaIndex, Workspace as AnalysisWorkspace,
    WorkspaceAnalysis,
};

/// A tracked document in the workspace.
#[derive(Debug, Clone)]
pub struct Document {
    /// The document's URI, its identity in the workspace.
    pub uri: Url,
    /// The document's current full text.
    pub text: String,
}

/// The cached whole-workspace analysis over all `.surql` documents, plus the
/// bookkeeping the per-URI methods need to map an analysis source id back to a
/// document. Valid only while [`SurqlCache::key`] matches the current input
/// hash; a mismatch forces a recompute so a stale analysis is never served.
#[derive(Debug)]
struct SurqlCache {
    /// Hash of the sorted `(uri, text)` set of every `.surql` document — the
    /// exact inputs [`analyze_workspace`] consumes.
    key: u64,
    /// The whole-workspace analysis result.
    analysis: WorkspaceAnalysis,
    /// Every `.surql` document in analysis order, with the source id it was
    /// registered under. Lets a request find its target source and resolve a
    /// definition span in any file back to a document.
    sources: Vec<(SourceId, Url, String)>,
}

impl SurqlCache {
    /// The `(source-id-string -> (uri, text))` map keyed for
    /// related-information / definition resolution.
    fn texts(&self) -> BTreeMap<String, (Url, String)> {
        self.sources
            .iter()
            .map(|(id, uri, text)| (id.to_string(), (uri.clone(), text.clone())))
            .collect()
    }

    /// Same mapping as [`Self::texts`], as a `HashMap` for the feature path.
    fn source_map(&self) -> HashMap<String, (Url, String)> {
        self.sources
            .iter()
            .map(|(id, uri, text)| (id.to_string(), (uri.clone(), text.clone())))
            .collect()
    }

    /// The source id a document was registered under, if it is tracked.
    fn source_for(&self, uri: &Url) -> Option<&SourceId> {
        self.sources
            .iter()
            .find(|(_, doc_uri, _)| doc_uri == uri)
            .map(|(id, _, _)| id)
    }
}

/// Cached diagnostics for a single host (TypeScript/Svelte) document. Keyed by
/// the pair `(surql-set hash, host-text hash)` so it stays valid only while
/// both the schema and the host's own text are unchanged.
#[derive(Debug)]
struct HostCache {
    /// `(surql-set hash, host-text hash)`.
    key: (u64, u64),
    /// Findings re-spanned onto the host file.
    diagnostics: Vec<Finding>,
    /// Source-id / host mapping for rendering.
    texts: BTreeMap<String, (Url, String)>,
}

/// The workspace tracks all open/saved documents and provides analysis through
/// the shared workspace facade, memoized so an unchanged document state is
/// analyzed at most once.
#[derive(Debug, Default)]
pub struct Workspace {
    /// All tracked documents, keyed by URI.
    documents: HashMap<Url, Document>,
    /// Workspace root folders.
    pub roots: Vec<PathBuf>,
    /// Memoized whole-workspace analysis over the `.surql` documents. Interior
    /// mutability so the `&self` analysis methods (called under the backend's
    /// `RwLock` read guard) can populate it; the `Mutex` serializes concurrent
    /// readers so the pass runs once even under a burst of feature requests.
    surql_cache: Mutex<Option<SurqlCache>>,
    /// Per-host-document diagnostics cache, keyed by URI.
    host_cache: Mutex<HashMap<Url, HostCache>>,
    /// Number of full [`analyze_workspace`] passes actually executed (cache
    /// misses). Observability + a test hook proving the cache is reused.
    analyze_runs: AtomicU64,
}

impl Workspace {
    /// Creates an empty workspace with no tracked documents or roots.
    pub fn new() -> Self {
        Self::default()
    }

    /// Update a document's content on open or change. The next analysis
    /// request recomputes because the input hash changes.
    pub fn upsert(&mut self, uri: Url, text: String) {
        self.documents.insert(uri.clone(), Document { uri, text });
    }

    /// Remove a document on close. Drops any cached host analysis for it; the
    /// `.surql` cache invalidates by hash on the next request.
    pub fn remove(&mut self, uri: &Url) {
        self.documents.remove(uri);
        if let Ok(cache) = self.host_cache.get_mut() {
            cache.remove(uri);
        }
    }

    /// Get all documents.
    pub fn documents(&self) -> impl Iterator<Item = &Document> {
        self.documents.values()
    }

    /// Number of full `analyze_workspace` passes executed so far. Increments
    /// only on a cache miss, so a stable count across requests proves the
    /// cache was reused.
    pub fn analyze_run_count(&self) -> u64 {
        self.analyze_runs.load(Ordering::Relaxed)
    }

    /// The current input hash: the sorted `(uri, text)` set of every tracked
    /// `.surql` document. Any add, remove, or edit changes it.
    fn surql_key(&self) -> u64 {
        let mut documents: Vec<_> = self
            .documents
            .values()
            .filter(|doc| is_surrealql_uri(&doc.uri))
            .collect();
        documents.sort_by(|left, right| left.uri.as_str().cmp(right.uri.as_str()));

        let mut hasher = DefaultHasher::new();
        for doc in documents {
            doc.uri.as_str().hash(&mut hasher);
            doc.text.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Builds the analysis workspace from every tracked `.surql` document (the
    /// schema + queries), returning it alongside the `(source id, uri, text)`
    /// of each document in analysis order.
    fn build_surql_workspace(&self) -> (AnalysisWorkspace, Vec<(SourceId, Url, String)>) {
        let mut analysis_workspace = AnalysisWorkspace::default();

        let mut documents: Vec<_> = self
            .documents
            .values()
            .filter(|doc| is_surrealql_uri(&doc.uri))
            .collect();
        documents.sort_by(|left, right| left.uri.as_str().cmp(right.uri.as_str()));

        let mut sources = Vec::with_capacity(documents.len());
        for doc in documents {
            let source_id = match doc.uri.to_file_path() {
                Ok(path) => analysis_workspace.add_file_source(path, doc.text.clone()),
                Err(_) => {
                    analysis_workspace.add_virtual_source(doc.uri.to_string(), doc.text.clone())
                }
            };
            sources.push((source_id, doc.uri.clone(), doc.text.clone()));
        }

        (analysis_workspace, sources)
    }

    /// Ensures the `.surql` analysis cache is populated for the current
    /// document state and runs `read` against it. `analyze_workspace` executes
    /// at most once per unchanged state: the first caller under a given input
    /// hash computes and stores; every later caller (until the next edit)
    /// reuses the cached result. The `Mutex` is held across `read`, which only
    /// clones the small slices each request needs.
    fn with_surql_cache<R>(&self, read: impl FnOnce(&SurqlCache) -> R) -> R {
        let key = self.surql_key();
        let mut guard = self
            .surql_cache
            .lock()
            .expect("surql analysis cache mutex poisoned");

        if guard.as_ref().is_none_or(|cache| cache.key != key) {
            let (analysis_workspace, sources) = self.build_surql_workspace();
            let analysis = analyze_workspace(&analysis_workspace);
            self.analyze_runs.fetch_add(1, Ordering::Relaxed);
            *guard = Some(SurqlCache {
                key,
                analysis,
                sources,
            });
        }

        read(guard.as_ref().expect("cache populated above"))
    }

    /// Analyze a document through the shared `surrealguard-workspace`
    /// pipeline. Plain `.surql` documents analyze as themselves (through the
    /// shared cache); host documents (TypeScript, Svelte, ...) analyze their
    /// embedded `surql` templates, with findings re-spanned onto the host file.
    pub fn diagnostic_analysis(&self, uri: &Url) -> Option<DiagnosticAnalysisResult> {
        let target = self.documents.get(uri)?;

        if target.text.trim().is_empty() {
            return Some(DiagnosticAnalysisResult {
                diagnostics: Vec::new(),
                source: target.text.clone(),
                texts: BTreeMap::new(),
            });
        }

        // Host target: keep the host-specific path (embedded extraction).
        if !is_surrealql_uri(uri) {
            return self.host_diagnostic_analysis(uri, target);
        }

        // `.surql` target: reuse the shared whole-workspace analysis.
        let source = target.text.clone();
        self.with_surql_cache(|cache| {
            let target_source = cache.source_for(uri)?;
            let diagnostics = cache
                .analysis
                .sources
                .get(target_source)
                .map(|output| output.diagnostics.clone())
                .unwrap_or_default();
            Some(DiagnosticAnalysisResult {
                diagnostics,
                source,
                texts: cache.texts(),
            })
        })
    }

    /// Diagnostics for a host document. Each embedded query is analyzed against
    /// the `.surql` schema and its findings re-spanned onto the host file.
    /// Cached per host URI keyed by `(surql-set hash, host-text hash)`, so an
    /// unchanged state does not re-run the pass, and a change to either the
    /// schema or the host text invalidates it.
    fn host_diagnostic_analysis(
        &self,
        uri: &Url,
        target: &Document,
    ) -> Option<DiagnosticAnalysisResult> {
        let surql_key = self.surql_key();
        let host_hash = {
            let mut hasher = DefaultHasher::new();
            target.text.hash(&mut hasher);
            hasher.finish()
        };
        let key = (surql_key, host_hash);

        // Cache hit: reuse without re-analyzing.
        if let Ok(cache) = self.host_cache.lock() {
            if let Some(entry) = cache.get(uri) {
                if entry.key == key {
                    return Some(DiagnosticAnalysisResult {
                        diagnostics: entry.diagnostics.clone(),
                        source: target.text.clone(),
                        texts: entry.texts.clone(),
                    });
                }
            }
        }

        // Miss: rebuild the `.surql` schema workspace, add this host's
        // embedded queries as virtual sources, and analyze once.
        let (mut analysis_workspace, surql_sources) = self.build_surql_workspace();
        let mut texts: BTreeMap<String, (Url, String)> = surql_sources
            .iter()
            .map(|(id, doc_uri, text)| (id.to_string(), (doc_uri.clone(), text.clone())))
            .collect();

        let embedded = surrealguard_embed::extract(uri.path(), &target.text);
        let mut queries = Vec::new();
        for (index, query) in embedded.into_iter().enumerate() {
            let source_id = analysis_workspace.add_virtual_source(
                format!("embedded://{}#{index}", uri.as_str()),
                query.text.clone(),
            );
            queries.push((source_id, query));
        }
        texts.insert(uri.to_string(), (uri.clone(), target.text.clone()));

        let workspace_output = analyze_workspace(&analysis_workspace);
        self.analyze_runs.fetch_add(1, Ordering::Relaxed);

        let host_source = SourceId::new(uri.to_string());
        let mut diagnostics = Vec::new();
        for (source_id, query) in &queries {
            let Some(source_output) = workspace_output.sources.get(source_id) else {
                continue;
            };
            for finding in &source_output.diagnostics {
                diagnostics.push(respan_to_host(finding, query, &host_source));
            }
        }

        if let Ok(mut cache) = self.host_cache.lock() {
            cache.insert(
                uri.clone(),
                HostCache {
                    key,
                    diagnostics: diagnostics.clone(),
                    texts: texts.clone(),
                },
            );
        }

        Some(DiagnosticAnalysisResult {
            diagnostics,
            source: target.text.clone(),
            texts,
        })
    }

    /// Analyze every tracked `.surql` document and return each document's
    /// findings keyed by URI. Runs the shared whole-workspace pass at most once
    /// (through the cache) instead of the per-file [`Self::diagnostic_analysis`]
    /// which would re-analyze the whole workspace for each file.
    pub fn analyze_all(&self) -> Vec<(Url, DiagnosticAnalysisResult)> {
        self.with_surql_cache(|cache| {
            let texts = cache.texts();
            cache
                .sources
                .iter()
                .map(|(source_id, uri, text)| {
                    let diagnostics = cache
                        .analysis
                        .sources
                        .get(source_id)
                        .map(|output| output.diagnostics.clone())
                        .unwrap_or_default();
                    (
                        uri.clone(),
                        DiagnosticAnalysisResult {
                            diagnostics,
                            source: text.clone(),
                            texts: texts.clone(),
                        },
                    )
                })
                .collect()
        })
    }

    /// Full analysis of a `.surql` document for editor features (inlay
    /// hints, hover): the target source's analysis output, the shared
    /// schema, its analysis source id, and its text. Returns `None` for
    /// empty documents and for non-`.surql` host files (whose embedded
    /// queries feed diagnostics only). Served from the shared cache.
    pub fn feature_analysis(&self, uri: &Url) -> Option<FeatureAnalysis> {
        let target = self.documents.get(uri)?;
        if target.text.trim().is_empty() {
            return None;
        }
        if !is_surrealql_uri(uri) {
            return None;
        }

        let text = target.text.clone();
        self.with_surql_cache(|cache| {
            let target_source = cache.source_for(uri)?.clone();
            let output = cache.analysis.sources.get(&target_source)?.clone();
            Some(FeatureAnalysis {
                output,
                schema: cache.analysis.schema.clone(),
                source: target_source,
                text,
                sources: cache.source_map(),
            })
        })
    }

    /// Scan workspace folders for `.surql` and `.surrealql` files and load them.
    pub fn scan_folders(&mut self) {
        for root in &self.roots.clone() {
            for entry in walkdir::WalkDir::new(root)
                .into_iter()
                .filter_map(std::result::Result::ok)
            {
                let path = entry.path();
                let is_surrealql = path
                    .extension()
                    .is_some_and(|extension| extension == "surql" || extension == "surrealql");

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
        ByteRange::new(host.start as u32, host.end as u32).expect("host spans are ordered"),
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

/// Full per-document analysis for editor features, carrying everything
/// the inlay-hint and hover handlers need to resolve types by span.
pub struct FeatureAnalysis {
    /// The target document's analysis output (statements, params).
    pub output: AnalysisOutput,
    /// The schema shared across all `.surql` documents in the workspace.
    pub schema: SchemaIndex,
    /// The target document's analysis source id.
    pub source: SourceId,
    /// The target document's full text, for offset/position conversion.
    pub text: String,
    /// Every tracked `.surql` document keyed by its analysis source id
    /// (stringified), for resolving a definition span that points into another
    /// file back to its URI and text.
    pub sources: HashMap<String, (Url, String)>,
}

/// Diagnostics-only result from the shared workspace analysis facade.
pub struct DiagnosticAnalysisResult {
    /// Findings for the target document, spanned into its own file.
    pub diagnostics: Vec<Finding>,
    /// The target document's full text, for rendering diagnostics.
    pub source: String,
    /// Every analyzed document keyed by its analysis source id, for
    /// resolving related-information spans that point at other files.
    pub texts: BTreeMap<String, (Url, String)>,
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

    fn workspace_with_schema_and_query() -> (Workspace, Url, Url) {
        let mut workspace = Workspace::new();
        let schema = Url::parse("file:///workspace/schema.surql").expect("valid uri");
        let query = Url::parse("file:///workspace/query.surql").expect("valid uri");
        workspace.upsert(
            schema.clone(),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;".into(),
        );
        workspace.upsert(query.clone(), "SELECT name FROM person;".into());
        (workspace, schema, query)
    }

    #[test]
    fn surql_analysis_is_computed_once_and_reused_across_requests() {
        let (workspace, schema, query) = workspace_with_schema_and_query();

        assert_eq!(workspace.analyze_run_count(), 0);

        // First request populates the cache with a single pass.
        let _ = workspace
            .diagnostic_analysis(&query)
            .expect("query analyzed");
        assert_eq!(
            workspace.analyze_run_count(),
            1,
            "first analysis runs exactly one workspace pass"
        );

        // Every other request at the same document state reuses the cache:
        // no additional pass runs, regardless of which method or file.
        let _ = workspace.feature_analysis(&query).expect("features");
        let _ = workspace.feature_analysis(&schema).expect("features");
        let _ = workspace
            .diagnostic_analysis(&schema)
            .expect("schema diagnostics");
        let _ = workspace.analyze_all();
        assert_eq!(
            workspace.analyze_run_count(),
            1,
            "unchanged document state is analyzed at most once"
        );
    }

    #[test]
    fn document_change_invalidates_the_cache() {
        let (mut workspace, _schema, query) = workspace_with_schema_and_query();

        let _ = workspace.feature_analysis(&query).expect("features");
        assert_eq!(workspace.analyze_run_count(), 1);

        // An edit changes the input hash -> the next request recomputes.
        workspace.upsert(query.clone(), "SELECT name FROM person WHERE name != NONE;".into());
        let _ = workspace
            .diagnostic_analysis(&query)
            .expect("re-analyzed after edit");
        assert_eq!(
            workspace.analyze_run_count(),
            2,
            "an edit forces exactly one recompute"
        );

        // Re-upserting identical text keeps the same hash -> cache hit.
        workspace.upsert(query.clone(), "SELECT name FROM person WHERE name != NONE;".into());
        let _ = workspace.feature_analysis(&query).expect("features");
        assert_eq!(
            workspace.analyze_run_count(),
            2,
            "re-upserting identical text does not recompute"
        );
    }

    #[test]
    fn removing_a_document_invalidates_the_cache() {
        let (mut workspace, schema, query) = workspace_with_schema_and_query();

        let _ = workspace.analyze_all();
        assert_eq!(workspace.analyze_run_count(), 1);

        workspace.remove(&query);
        let _ = workspace
            .diagnostic_analysis(&schema)
            .expect("schema still analyzed");
        assert_eq!(
            workspace.analyze_run_count(),
            2,
            "closing a document recomputes on the next request"
        );
    }

    #[test]
    fn host_document_analysis_is_cached_per_state() {
        let mut workspace = Workspace::new();
        let schema = Url::parse("file:///workspace/schema.surql").expect("valid uri");
        workspace.upsert(
            schema,
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;".into(),
        );
        let host = Url::parse("file:///workspace/app.ts").expect("valid uri");
        workspace.upsert(
            host.clone(),
            "const q = surql`SELECT name FROM person`;".into(),
        );

        // Warm the shared `.surql` schema pass so the count below reflects
        // only host recomputes.
        let _ = workspace.analyze_all();
        let base = workspace.analyze_run_count();

        let _ = workspace.diagnostic_analysis(&host).expect("host analyzed");
        let after_first = workspace.analyze_run_count();
        assert!(
            after_first > base,
            "first host analysis runs a pass ({base} -> {after_first})"
        );

        // Same host + schema state -> cache hit, no new pass.
        let _ = workspace.diagnostic_analysis(&host).expect("host reused");
        assert_eq!(
            workspace.analyze_run_count(),
            after_first,
            "unchanged host state reuses the cached analysis"
        );

        // Editing the host text invalidates its entry.
        workspace.upsert(
            host.clone(),
            "const q = surql`SELECT name FROM person WHERE name != NONE`;".into(),
        );
        let _ = workspace.diagnostic_analysis(&host).expect("host re-analyzed");
        assert_eq!(
            workspace.analyze_run_count(),
            after_first + 1,
            "editing the host text forces one recompute"
        );
    }
}
