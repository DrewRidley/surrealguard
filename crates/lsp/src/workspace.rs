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
//! subsequent request reuses the cached [`surrealguard_workspace::WorkspaceAnalysis`]. Any edit
//! (`upsert`) or close (`remove`) changes the key, so the next request
//! recomputes — the cache can never serve analysis that predates the latest
//! edit.
//!
//! # Text is shared, never copied
//!
//! Document text lives in one `Arc<str>` per document. The cache, every
//! per-request result, and the `(source id → uri, text)` map diagnostics use
//! to resolve related spans all hold references into those same allocations,
//! so a request costs pointers, not bytes. This is load-bearing: a workspace
//! of 500 files (2.4 MB of text) used to reach 900 MB resident on
//! `initialized`, because a sweep built one fresh copy of *every* text for
//! *each* document it published.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tower_lsp::lsp_types::Url;

use surrealguard_diagnostics::Finding;
use surrealguard_syntax::parse::{parse_source, ParsedSource};
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};
use surrealguard_workspace::analysis::{
    build_workspace_schema, changed_symbols, reanalyze_sources, source_reference_set,
    source_requires_full_reanalysis, sources_with_changed_cycle_findings, SourceReferenceSet,
};
use surrealguard_workspace::config::{SourceConfig, WorkspaceConfig};
use surrealguard_workspace::{
    analyze_one_source, analyze_workspace, build_global_catalog, AnalysisOutput, GlobalCatalog,
    SchemaIndex, Workspace as AnalysisWorkspace,
};

use crate::text::LineIndex;

/// Every analyzed document's `(uri, text)` keyed by its analysis source id
/// (stringified). Diagnostics resolve related-information spans through it
/// and go-to-definition resolves a target file's URI through it. Built once
/// per analysis state and shared by `Arc`: handing it to a request is O(1)
/// and adds no bytes.
pub type SourceTexts = Arc<BTreeMap<String, (Url, Arc<str>)>>;

/// A tracked document in the workspace.
#[derive(Debug, Clone)]
pub struct Document {
    /// The document's URI, its identity in the workspace.
    pub uri: Url,
    /// The document's current full text, shared with every cache entry and
    /// request result that refers to it.
    pub text: Arc<str>,
    /// Hash of `text`, computed once on upsert so the cache keys are built
    /// from one word per document instead of re-hashing every byte of every
    /// document on every request.
    text_hash: u64,
    /// Whether the text can contribute to the schema/catalog, decided once on
    /// upsert (see [`is_schema_relevant`]). Only meaningful for `.surql`
    /// documents; host files never contribute.
    schema_relevant: bool,
    /// The line index over `text`, built on the first conversion this version
    /// of the document needs and reused by every later one. An edit replaces
    /// the whole `Document`, so this can never outlive the text it indexes.
    index: OnceLock<Arc<LineIndex>>,
}

impl Document {
    fn new(uri: Url, text: String) -> Self {
        let text_hash = {
            let mut hasher = DefaultHasher::new();
            text.hash(&mut hasher);
            hasher.finish()
        };
        let schema_relevant = is_surrealql_uri(&uri) && is_schema_relevant(&text);
        Self {
            uri,
            text: Arc::from(text),
            text_hash,
            schema_relevant,
            index: OnceLock::new(),
        }
    }

    /// The document's line index, built once per text version.
    ///
    /// Every whole-document surface — semantic tokens, diagnostics, inlay
    /// hints — converts one span per item it emits. Scanning the text per
    /// span makes each of those quadratic in file size, so they all share
    /// this one index instead.
    pub fn line_index(&self) -> Arc<LineIndex> {
        Arc::clone(
            self.index
                .get_or_init(|| Arc::new(LineIndex::new(Arc::clone(&self.text)))),
        )
    }
}

/// The cached whole-workspace analysis over all `.surql` documents, plus the
/// bookkeeping the per-URI methods need to map an analysis source id back to a
/// document. Valid only while [`SurqlCache::key`] matches the current input
/// hash; a mismatch forces a recompute so a stale analysis is never served.
#[derive(Debug)]
struct SurqlCache {
    /// Hash of the sorted `(uri, text)` set of every `.surql` document — the
    /// exact inputs [`analyze_workspace`] consumes. A mismatch forces a
    /// recompute (which may be the incremental fast path).
    key: u64,
    /// Hash of the schema-defining inputs: every `.surql` URI (so add/remove/
    /// reorder invalidates) plus the *text* of only the schema-relevant
    /// documents (those containing a DEFINE/REMOVE/ALTER/CREATE/UPSERT/INSERT/
    /// DELETE). Editing a pure query document leaves this unchanged, which is
    /// what makes the cached [`GlobalCatalog`] reusable across such an edit.
    global_key: u64,
    /// The reusable cross-source catalog behind this analysis, cached so a
    /// query-only edit can re-analyze just the dirty document against it, and so
    /// a schema edit can diff it symbol-by-symbol (see
    /// [`Workspace::try_symbol_incremental`]).
    catalog: GlobalCatalog,
    /// Per-source analysis output — [`surrealguard_workspace::WorkspaceAnalysis::sources`], with each
    /// output behind an `Arc` so an incremental rebuild carries the unchanged
    /// ones forward by pointer and a request borrows one without copying it.
    outputs: BTreeMap<SourceId, Arc<AnalysisOutput>>,
    /// The schema index — [`surrealguard_workspace::WorkspaceAnalysis::schema`] — shared with every
    /// feature request rather than cloned into each.
    schema: Arc<SchemaIndex>,
    /// Every `.surql` document in analysis order, with the source id it was
    /// registered under. Lets a request find its target source and resolve a
    /// definition span in any file back to a document.
    sources: Vec<(SourceId, Url, Arc<str>)>,
    /// [`Self::sources`] as the shared lookup map, built once here.
    texts: SourceTexts,
    /// Source id by document URI, for the per-URI entry points.
    by_uri: HashMap<Url, SourceId>,
    /// The parsed source for each document, cached so an incremental edit
    /// re-parses only the documents whose text changed and reuses these for the
    /// rest. Missing only for documents that failed to parse.
    parsed: BTreeMap<SourceId, Arc<ParsedSource>>,
    /// Each source's conservative catalog-symbol reference set, cached so the
    /// symbol-incremental path re-walks only the dirty documents.
    reference_sets: BTreeMap<SourceId, SourceReferenceSet>,
}

impl SurqlCache {
    /// Assembles a cache entry, deriving the lookup structures from `sources`.
    #[allow(clippy::too_many_arguments)]
    fn new(
        key: u64,
        global_key: u64,
        catalog: GlobalCatalog,
        outputs: BTreeMap<SourceId, Arc<AnalysisOutput>>,
        schema: Arc<SchemaIndex>,
        sources: Vec<(SourceId, Url, Arc<str>)>,
        parsed: BTreeMap<SourceId, Arc<ParsedSource>>,
        reference_sets: BTreeMap<SourceId, SourceReferenceSet>,
    ) -> Self {
        let texts = Arc::new(
            sources
                .iter()
                .map(|(id, uri, text)| (id.to_string(), (uri.clone(), Arc::clone(text))))
                .collect(),
        );
        let by_uri = sources
            .iter()
            .map(|(id, uri, _)| (uri.clone(), id.clone()))
            .collect();
        Self {
            key,
            global_key,
            catalog,
            outputs,
            schema,
            sources,
            texts,
            by_uri,
            parsed,
            reference_sets,
        }
    }

    /// The shared `(source-id-string -> (uri, text))` map. O(1): one `Arc`
    /// clone.
    fn texts(&self) -> SourceTexts {
        Arc::clone(&self.texts)
    }

    /// The source id a document was registered under, if it is tracked.
    fn source_for(&self, uri: &Url) -> Option<&SourceId> {
        self.by_uri.get(uri)
    }

    /// The cached text of each source by id, for diffing against a new
    /// document state.
    fn text_by_id(&self) -> HashMap<&SourceId, &Arc<str>> {
        self.sources
            .iter()
            .map(|(id, _, text)| (id, text))
            .collect()
    }
}

/// One embedded query of a host document, with the analysis it produced.
/// Diagnostics read the output's findings (re-spanned onto the host);
/// cursor-addressed features map the cursor into `query` and answer from the
/// same output, so both surfaces see one analysis.
#[derive(Debug)]
struct HostQuery {
    /// The extraction: query text plus the map back to host offsets.
    query: surrealguard_embed::EmbeddedQuery,
    /// The virtual source id the query was analyzed under.
    source: SourceId,
    /// The query's analysis output, in *embedded* coordinates.
    output: Arc<AnalysisOutput>,
    /// The query's parse, kept so a cursor-addressed request reads the tree
    /// instead of re-parsing the query. `None` when the query did not parse.
    parsed: Option<Arc<ParsedSource>>,
}

/// Cached analysis for a single host (TypeScript/Svelte) document. Keyed by
/// the pair `(surql-set hash, host-text hash)` so it stays valid only while
/// both the schema and the host's own text are unchanged. Held behind an
/// `Arc` so a cache hit hands out a pointer, not a copy of every query's
/// analysis.
#[derive(Debug)]
struct HostCache {
    /// `(surql-set hash, host-text hash)`.
    key: (u64, u64),
    /// Every embedded query with its own analysis.
    queries: Vec<HostQuery>,
    /// Findings re-spanned onto the host file.
    diagnostics: Vec<Finding>,
    /// Source-id / host mapping for rendering.
    texts: SourceTexts,
}

/// The workspace tracks all open/saved documents and provides analysis through
/// the shared workspace facade, memoized so an unchanged document state is
/// analyzed at most once.
#[derive(Debug, Default)]
pub struct Workspace {
    /// All tracked documents, keyed by URI. A `BTreeMap` so iteration is
    /// already in analysis (URI) order — the order the cache keys, the source
    /// ids, and the analysis itself are all sensitive to.
    documents: BTreeMap<Url, Document>,
    /// Workspace root folders.
    pub roots: Vec<PathBuf>,
    /// The resolved `surrealguard.toml`, so `[analysis]` (the target
    /// SurrealDB version behind the 8xxx checks) and `[diagnostics]` steer
    /// the editor's analysis exactly as they steer `surrealguard check`.
    /// Defaults until [`Self::set_config`] installs the workspace's own.
    config: WorkspaceConfig,
    /// Memoized whole-workspace analysis over the `.surql` documents. Interior
    /// mutability so the `&self` analysis methods (called under the backend's
    /// `RwLock` read guard) can populate it; the `Mutex` serializes concurrent
    /// readers so the pass runs once even under a burst of feature requests.
    surql_cache: Mutex<Option<SurqlCache>>,
    /// Per-host-document diagnostics cache, keyed by URI.
    host_cache: Mutex<HashMap<Url, Arc<HostCache>>>,
    /// Number of full [`analyze_workspace`] passes actually executed (cache
    /// misses that rebuilt the whole workspace). Observability + a test hook
    /// proving the cache is reused.
    analyze_runs: AtomicU64,
    /// Number of incremental single-source re-analyses (the fast path: a
    /// query-only edit that reused the cached [`GlobalCatalog`] instead of
    /// re-running the whole-workspace pass).
    incremental_runs: AtomicU64,
    /// Total number of sources actually re-analyzed across every incremental
    /// pass (query-only fast path plus symbol-incremental schema path). A
    /// granularity hook for tests: a schema edit that touches symbols nothing
    /// else references re-analyzes just the edited source, not the whole set.
    reanalyzed_sources: AtomicU64,
}

impl Workspace {
    /// Creates an empty workspace with no tracked documents or roots.
    pub fn new() -> Self {
        Self::default()
    }

    /// Installs the workspace's resolved `surrealguard.toml`. Every cached
    /// analysis is dropped: the config is an analysis input the cache keys do
    /// not cover, so a result computed under the old one must never be served
    /// under the new.
    pub fn set_config(&mut self, config: WorkspaceConfig) {
        self.config = config;
        if let Ok(cache) = self.surql_cache.get_mut() {
            *cache = None;
        }
        if let Ok(cache) = self.host_cache.get_mut() {
            cache.clear();
        }
    }

    /// The workspace's resolved configuration.
    pub fn config(&self) -> &WorkspaceConfig {
        &self.config
    }

    /// Update a document's content on open or change. The next analysis
    /// request recomputes because the input hash changes.
    pub fn upsert(&mut self, uri: Url, text: String) {
        self.documents.insert(uri.clone(), Document::new(uri, text));
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

    /// A tracked document's current text.
    pub fn document_text(&self, uri: &Url) -> Option<Arc<str>> {
        self.documents.get(uri).map(|doc| Arc::clone(&doc.text))
    }

    /// A tracked document's line index, built once per text version and
    /// shared by every conversion against it.
    pub fn line_index(&self, uri: &Url) -> Option<Arc<LineIndex>> {
        self.documents.get(uri).map(Document::line_index)
    }

    /// Every tracked document the server publishes diagnostics for — `.surql`
    /// files first, then host files, each set in URI order — so a sweep can
    /// analyze and publish one document at a time instead of materializing
    /// every result up front. The first `.surql` request warms the shared
    /// cache; every later one, host files included, reads it.
    pub fn tracked_uris(&self) -> Vec<Url> {
        let surql = self
            .documents
            .keys()
            .filter(|uri| is_surrealql_uri(uri))
            .cloned();
        let hosts = self
            .documents
            .keys()
            .filter(|uri| is_host_uri(uri))
            .cloned();
        surql.chain(hosts).collect()
    }

    /// Number of full `analyze_workspace` passes executed so far. Increments
    /// only on a cache miss, so a stable count across requests proves the
    /// cache was reused.
    pub fn analyze_run_count(&self) -> u64 {
        self.analyze_runs.load(Ordering::Relaxed)
    }

    /// Number of incremental single-source re-analyses executed so far. A
    /// query-only edit increments this instead of [`Self::analyze_run_count`],
    /// proving the fast path reused the cached [`GlobalCatalog`].
    pub fn incremental_run_count(&self) -> u64 {
        self.incremental_runs.load(Ordering::Relaxed)
    }

    /// Total number of sources re-analyzed across all incremental passes so
    /// far. Lets a test assert that a schema edit re-ran only the sources that
    /// actually depend on the change, not the whole workspace.
    pub fn reanalyzed_source_count(&self) -> u64 {
        self.reanalyzed_sources.load(Ordering::Relaxed)
    }

    /// Every tracked `.surql` document in analysis (URI) order.
    fn surql_documents(&self) -> Vec<&Document> {
        self.documents
            .values()
            .filter(|doc| is_surrealql_uri(&doc.uri))
            .collect()
    }

    /// The current input hash: the sorted `(uri, text)` set of every tracked
    /// `.surql` document. Any add, remove, or edit changes it. Each text
    /// enters through the hash computed at upsert, so this is O(documents),
    /// not O(bytes).
    fn surql_key(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        for doc in self.surql_documents() {
            doc.uri.as_str().hash(&mut hasher);
            doc.text_hash.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// The schema-defining input hash: every `.surql` URI in sorted order (so
    /// any add/remove/reorder invalidates it) plus the *text* of only the
    /// schema-relevant documents. A pure query document's text is excluded, so
    /// editing it does not change this key — that is precisely the condition
    /// under which the cached [`GlobalCatalog`] stays valid and the dirty file
    /// can be re-analyzed alone.
    fn global_key(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        for doc in self.surql_documents() {
            doc.uri.as_str().hash(&mut hasher);
            if doc.schema_relevant {
                doc.text_hash.hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    /// Builds the analysis workspace from every tracked `.surql` document (the
    /// schema + queries), returning it alongside the `(source id, uri, text)`
    /// of each document in analysis order. Only the full path needs this: the
    /// analysis workspace owns its own copy of every text (its registry stores
    /// `String`), so building it is the one place a rebuild copies text at
    /// all, and it is dropped as soon as the pass has run. The workspace
    /// carries the resolved config, so the pass reads the target version and
    /// the diagnostic settings the CLI reads.
    fn build_surql_workspace(
        &self,
        documents: &[&Document],
    ) -> (AnalysisWorkspace, Vec<(SourceId, Url, Arc<str>)>) {
        let mut analysis_workspace = AnalysisWorkspace::new(self.config.clone());
        let mut sources = Vec::with_capacity(documents.len());
        for doc in documents {
            let source_id = match doc.uri.to_file_path() {
                Ok(path) => analysis_workspace.add_file_source(path, doc.text.to_string()),
                Err(_) => {
                    analysis_workspace.add_virtual_source(doc.uri.to_string(), doc.text.to_string())
                }
            };
            sources.push((source_id, doc.uri.clone(), Arc::clone(&doc.text)));
        }
        (analysis_workspace, sources)
    }

    /// The `(source id, uri, text)` list for the current documents, reusing
    /// `prev`'s source ids. Valid only when the document set is unchanged —
    /// same URIs in the same order — which is also the precondition of both
    /// incremental paths: a source id is a function of the document's path
    /// (or, for a virtual document, of its position among the virtual ones),
    /// so an unchanged set keeps every id. `None` when the set changed, which
    /// sends the caller to the full path.
    fn sources_from_prev(
        prev: &SurqlCache,
        documents: &[&Document],
    ) -> Option<Vec<(SourceId, Url, Arc<str>)>> {
        if documents.len() != prev.sources.len() {
            return None;
        }
        prev.sources
            .iter()
            .zip(documents)
            .map(|((id, prev_uri, _), doc)| {
                (*prev_uri == doc.uri).then(|| (id.clone(), doc.uri.clone(), Arc::clone(&doc.text)))
            })
            .collect()
    }

    /// Ensures the `.surql` analysis cache is populated for the current
    /// document state and runs `read` against it. `analyze_workspace` executes
    /// at most once per unchanged state: the first caller under a given input
    /// hash computes and stores; every later caller (until the next edit)
    /// reuses the cached result. The `Mutex` is held across `read`, which only
    /// clones the `Arc`s each request needs.
    fn with_surql_cache<R>(&self, read: impl FnOnce(&SurqlCache) -> R) -> R {
        let key = self.surql_key();
        let mut guard = self
            .surql_cache
            .lock()
            .expect("surql analysis cache mutex poisoned");

        if guard.as_ref().is_none_or(|cache| cache.key != key) {
            let fresh = self.compute_surql_cache(guard.as_ref(), key);
            *guard = Some(fresh);
        }

        read(guard.as_ref().expect("cache populated above"))
    }

    /// Recomputes the `.surql` analysis cache for the current document state.
    /// Chooses the cheapest sound path:
    ///   * **query-only edit** (schema-defining inputs unchanged): re-analyze
    ///     only the dirty documents against the cached [`GlobalCatalog`].
    ///   * **schema edit** (a `DEFINE`/write changed): diff the catalog
    ///     symbol-by-symbol and re-analyze only the edited documents plus the
    ///     ones whose analysis actually reads a changed symbol.
    ///   * otherwise a full whole-workspace pass.
    ///
    /// Every path yields a result equivalent to `analyze_workspace` at the
    /// current state.
    fn compute_surql_cache(&self, prev: Option<&SurqlCache>, key: u64) -> SurqlCache {
        let documents = self.surql_documents();
        let global_key = self.global_key();
        let require = self.config.diagnostics.require_suppression_reasons;

        if let Some(prev) = prev {
            if let Some(sources) = Self::sources_from_prev(prev, &documents) {
                if prev.global_key == global_key {
                    // Query-only edit: the catalog is unchanged.
                    if let Some(fresh) =
                        self.try_incremental(prev, &sources, key, global_key, require)
                    {
                        return fresh;
                    }
                } else if let Some(fresh) =
                    self.try_symbol_incremental(prev, &sources, key, global_key, require)
                {
                    // Schema edit narrowed to the sources that depend on it.
                    return fresh;
                }
            }
        }

        // Full path: rebuild the whole-workspace analysis, the catalog, the
        // parsed set, and every source's reference set.
        let (analysis_workspace, sources) = self.build_surql_workspace(&documents);
        let mut analysis = analyze_workspace(&analysis_workspace);
        drop(analysis_workspace);
        let parsed = Self::parse_all(&sources);
        let ordered = Self::ordered_parsed(&sources, &parsed);
        let catalog = self.global_catalog(&ordered);
        let reference_sets = Self::reference_sets_for(&parsed);
        self.analyze_runs.fetch_add(1, Ordering::Relaxed);
        let schema = Arc::new(std::mem::take(&mut analysis.schema));
        let outputs = analysis
            .sources
            .into_iter()
            .map(|(id, output)| (id, Arc::new(output)))
            .collect();
        SurqlCache::new(
            key,
            global_key,
            catalog,
            outputs,
            schema,
            sources,
            parsed,
            reference_sets,
        )
    }

    /// The cross-source catalog over `ordered`, carrying the configured target
    /// SurrealDB version — a workspace-wide fact every single-source
    /// re-analysis reads from the catalog, so the 8xxx checks fire on the
    /// incremental paths exactly as on the full pass.
    fn global_catalog(&self, ordered: &[Arc<ParsedSource>]) -> GlobalCatalog {
        build_global_catalog(ordered).with_target_version(self.config.analysis.target_version())
    }

    /// Parses every document, keyed by source id. Documents that fail to parse
    /// are omitted — exactly the set [`analyze_workspace`] feeds to the catalog.
    /// Each parse shares the document's text (`parse_source` takes an
    /// `Arc<str>`), so the parsed set adds trees, not another copy of the
    /// workspace.
    fn parse_all(sources: &[(SourceId, Url, Arc<str>)]) -> BTreeMap<SourceId, Arc<ParsedSource>> {
        sources
            .iter()
            .filter_map(|(id, _uri, text)| {
                parse_source(id.clone(), Arc::clone(text))
                    .ok()
                    .map(|parsed| (id.clone(), Arc::new(parsed)))
            })
            .collect()
    }

    /// The successfully-parsed sources in analysis (document) order — the order
    /// the pre-passes and the schema walk are sensitive to.
    fn ordered_parsed(
        sources: &[(SourceId, Url, Arc<str>)],
        parsed: &BTreeMap<SourceId, Arc<ParsedSource>>,
    ) -> Vec<Arc<ParsedSource>> {
        sources
            .iter()
            .filter_map(|(id, _uri, _text)| parsed.get(id).cloned())
            .collect()
    }

    /// The conservative reference set for each parsed source.
    fn reference_sets_for(
        parsed: &BTreeMap<SourceId, Arc<ParsedSource>>,
    ) -> BTreeMap<SourceId, SourceReferenceSet> {
        parsed
            .iter()
            .map(|(id, parsed)| (id.clone(), source_reference_set(parsed)))
            .collect()
    }

    /// Whether a source's text is unchanged from the cached one. An unchanged
    /// document keeps its `Arc`, so this is a pointer compare in the common
    /// case and a byte compare only for a re-upsert of identical text.
    fn same_text(previous: Option<&&Arc<str>>, current: &Arc<str>) -> bool {
        previous.is_some_and(|previous| Arc::ptr_eq(previous, current) || ***previous == **current)
    }

    /// Attempts to build the new cache incrementally from `prev`: re-analyze
    /// only the documents whose text changed (all of which are query-only,
    /// since the schema-defining inputs matched), reusing `prev`'s catalog,
    /// schema, and every unchanged source's output. Returns `None` — forcing
    /// the full fallback — if any dirty document fails to parse cleanly, so a
    /// divergent case is never served stale. The caller guarantees `sources`
    /// names the same documents as `prev` (see [`Self::sources_from_prev`]).
    fn try_incremental(
        &self,
        prev: &SurqlCache,
        sources: &[(SourceId, Url, Arc<str>)],
        key: u64,
        global_key: u64,
        require_suppression_reasons: bool,
    ) -> Option<SurqlCache> {
        let prev_text = prev.text_by_id();

        let mut outputs = prev.outputs.clone();
        let mut parsed = prev.parsed.clone();
        let mut reference_sets = prev.reference_sets.clone();
        let mut dirty = 0usize;
        for (id, _uri, text) in sources {
            if Self::same_text(prev_text.get(id), text) {
                continue;
            }

            // Re-analyze this dirty (query-only) source against the cached
            // catalog. A hard parse failure has no `ParsedSource`; bail so the
            // full pass emits the parse-error finding exactly as before.
            let parsed_source = Arc::new(parse_source(id.clone(), Arc::clone(text)).ok()?);
            let output =
                analyze_one_source(&prev.catalog, &parsed_source, require_suppression_reasons);
            reference_sets.insert(id.clone(), source_reference_set(&parsed_source));
            parsed.insert(id.clone(), parsed_source);
            outputs.insert(id.clone(), Arc::new(output));
            dirty += 1;
        }

        // The schema is unchanged: a query-only source contributes no
        // DEFINE/REMOVE/ALTER, so `prev`'s schema still holds.
        self.incremental_runs
            .fetch_add(dirty as u64, Ordering::Relaxed);
        self.reanalyzed_sources
            .fetch_add(dirty as u64, Ordering::Relaxed);

        Some(SurqlCache::new(
            key,
            global_key,
            prev.catalog.clone(),
            outputs,
            Arc::clone(&prev.schema),
            sources.to_vec(),
            parsed,
            reference_sets,
        ))
    }

    /// Attempts a symbol-level incremental rebuild for a SCHEMA edit: rebuild
    /// the cheap [`GlobalCatalog`] (O(defines)), diff it against `prev`'s to
    /// learn which catalog symbols changed, and re-analyze only the dirty
    /// documents plus the ones whose analysis reads a changed symbol. Every
    /// unaffected source keeps its previous output verbatim.
    ///
    /// Returns `None` — forcing the full fallback — when it cannot guarantee an
    /// output identical to `analyze_workspace`:
    ///   * a dirty document fails to parse, or
    ///   * a dirty document carries an unmodeled schema effect
    ///     (`REMOVE`/`ALTER`/`DEFINE PARAM`/`DEFINE ANALYZER`, per
    ///     [`source_requires_full_reanalysis`]).
    ///
    /// A changed document set (add/remove/reorder) can shift analysis order
    /// and cross-source visibility beyond what symbol diffing models; the
    /// caller never gets here in that case (see [`Self::sources_from_prev`]).
    fn try_symbol_incremental(
        &self,
        prev: &SurqlCache,
        sources: &[(SourceId, Url, Arc<str>)],
        key: u64,
        global_key: u64,
        require_suppression_reasons: bool,
    ) -> Option<SurqlCache> {
        let prev_text = prev.text_by_id();

        // Re-parse only the dirty documents; reuse the cached `ParsedSource` for
        // the rest. Track the parsed set both in analysis order (for the
        // pre-passes) and by id (for the new cache).
        let mut parsed: BTreeMap<SourceId, Arc<ParsedSource>> = BTreeMap::new();
        let mut ordered: Vec<Arc<ParsedSource>> = Vec::new();
        let mut dirty: BTreeSet<SourceId> = BTreeSet::new();
        for (id, _uri, text) in sources {
            let parsed_source = if Self::same_text(prev_text.get(id), text) {
                // Unchanged: reuse the cached parse, or skip if it never
                // parsed (its cached output — a parse error — still holds).
                match prev.parsed.get(id) {
                    Some(cached) => cached.clone(),
                    None => continue,
                }
            } else {
                dirty.insert(id.clone());
                // A dirty document that fails to parse would change the
                // catalog and emit a parse-error finding: only the full pass
                // reproduces that.
                Arc::new(parse_source(id.clone(), Arc::clone(text)).ok()?)
            };
            parsed.insert(id.clone(), parsed_source.clone());
            ordered.push(parsed_source);
        }

        // Order-sensitive / value-carrying schema effects the symbol diff cannot
        // model: fall back to the full pass.
        for id in &dirty {
            if let Some(parsed_source) = parsed.get(id) {
                if source_requires_full_reanalysis(parsed_source) {
                    return None;
                }
            }
        }

        let new_catalog = self.global_catalog(&ordered);
        let changed = changed_symbols(&prev.catalog, &new_catalog);

        // Reference sets: reuse the cached set for unchanged documents, re-walk
        // the dirty ones.
        let mut reference_sets: BTreeMap<SourceId, SourceReferenceSet> = BTreeMap::new();
        for (id, parsed_source) in &parsed {
            let refs = if dirty.contains(id) {
                source_reference_set(parsed_source)
            } else {
                prev.reference_sets
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| source_reference_set(parsed_source))
            };
            reference_sets.insert(id.clone(), refs);
        }

        // Affected = dirty ∪ sources reading a changed symbol ∪ sources whose
        // cross-source cycle (5009) findings changed.
        let mut affected: BTreeSet<SourceId> = dirty.clone();
        for (id, refs) in &reference_sets {
            if refs.is_affected_by(&changed) {
                affected.insert(id.clone());
            }
        }
        for id in sources_with_changed_cycle_findings(&prev.catalog, &new_catalog) {
            if parsed.contains_key(&id) {
                affected.insert(id);
            }
        }

        // Re-analyze exactly the affected sources against the fresh catalog,
        // reproducing the whole-workspace per-source output for each.
        let fresh_outputs = reanalyze_sources(
            &ordered,
            &new_catalog,
            &affected,
            require_suppression_reasons,
        );
        let reanalyzed = fresh_outputs.len() as u64;

        let mut outputs = prev.outputs.clone();
        for (id, output) in fresh_outputs {
            outputs.insert(id, Arc::new(output));
        }
        // The schema changed; rebuild it (cheap — no per-statement analysis) so
        // editor features resolve against the current catalog.
        let schema = Arc::new(build_workspace_schema(&ordered));

        self.incremental_runs
            .fetch_add(reanalyzed, Ordering::Relaxed);
        self.reanalyzed_sources
            .fetch_add(reanalyzed, Ordering::Relaxed);

        Some(SurqlCache::new(
            key,
            global_key,
            new_catalog,
            outputs,
            schema,
            sources.to_vec(),
            parsed,
            reference_sets,
        ))
    }

    /// Analyze a document through the shared `surrealguard-workspace`
    /// pipeline. Plain `.surql` documents analyze as themselves (through the
    /// shared cache); host documents (TypeScript, Svelte, ...) analyze their
    /// embedded queries, with findings re-spanned onto the host file.
    pub fn diagnostic_analysis(&self, uri: &Url) -> Option<DiagnosticAnalysisResult> {
        let target = self.documents.get(uri)?;

        if target.text.trim().is_empty() {
            return Some(DiagnosticAnalysisResult {
                diagnostics: Vec::new(),
                source: Arc::clone(&target.text),
                index: target.line_index(),
                texts: SourceTexts::default(),
            });
        }

        // Host target: keep the host-specific path (embedded extraction).
        if !is_surrealql_uri(uri) {
            let host = self.host_analysis(uri, target)?;
            return Some(DiagnosticAnalysisResult {
                diagnostics: host.diagnostics.clone(),
                source: Arc::clone(&target.text),
                index: target.line_index(),
                texts: Arc::clone(&host.texts),
            });
        }

        // `.surql` target: reuse the shared whole-workspace analysis.
        let source = Arc::clone(&target.text);
        let index = target.line_index();
        self.with_surql_cache(|cache| {
            let target_source = cache.source_for(uri)?;
            let diagnostics = cache
                .outputs
                .get(target_source)
                .map(|output| output.diagnostics.clone())
                .unwrap_or_default();
            Some(DiagnosticAnalysisResult {
                diagnostics,
                source,
                index: Arc::clone(&index),
                texts: cache.texts(),
            })
        })
    }

    /// The analysis of a host document's embedded queries, memoized per host
    /// URI under `(surql-set hash, host-text hash)`: unchanged state does no
    /// work, and a change to either the schema or the host's own text
    /// invalidates. Both the diagnostics surface and the cursor-addressed
    /// features read this one entry.
    ///
    /// A host file with no embedded query costs an extraction and nothing
    /// else — it never reaches the `.surql` analysis at all.
    fn host_analysis(&self, uri: &Url, target: &Document) -> Option<Arc<HostCache>> {
        // Only the file types [`surrealguard_embed::extract`] actually knows.
        // Anything else the client attached us to (JSON, Markdown, a lockfile)
        // would otherwise be parsed as TypeScript on every keystroke.
        if !is_host_uri(uri) {
            return None;
        }

        let host_hash = target.text_hash;

        let queries = surrealguard_embed::extract(uri.path(), &target.text);
        if queries.is_empty() {
            // Nothing embedded: an empty result, keyed so a later edit that
            // *adds* a query still recomputes. Deliberately keyed on the
            // `.surql` state we never read (0), so a schema edit alone does
            // not invalidate a host file that asks nothing of the schema.
            return Some(Arc::new(HostCache {
                key: (0, host_hash),
                queries: Vec::new(),
                diagnostics: Vec::new(),
                texts: SourceTexts::default(),
            }));
        }

        let key = (self.surql_key(), host_hash);

        // Cache hit: reuse without re-analyzing.
        if let Ok(cache) = self.host_cache.lock() {
            if let Some(entry) = cache.get(uri) {
                if entry.key == key {
                    return Some(Arc::clone(entry));
                }
            }
        }

        let host_source = SourceId::new(uri.to_string());
        let fresh = Arc::new(self.analyze_host_queries(uri, target, key, queries, &host_source));

        if let Ok(mut cache) = self.host_cache.lock() {
            cache.insert(uri.clone(), Arc::clone(&fresh));
        }
        Some(fresh)
    }

    /// Analyzes a host document's embedded queries against the workspace
    /// schema.
    ///
    /// Read-only queries — the overwhelming majority of what a client file
    /// holds — take the same fast path a `.surql` query edit takes: each is
    /// analyzed alone against the cached [`GlobalCatalog`], so a keystroke in
    /// a `.svelte` file costs one small query, not one whole-workspace pass.
    /// A query that could *change* the catalog (a `CREATE`, a `DEFINE`) breaks
    /// [`analyze_one_source`]'s contract, so those fall back to the full pass
    /// with the embedded sources registered — exactly the previous behavior.
    fn analyze_host_queries(
        &self,
        uri: &Url,
        target: &Document,
        key: (u64, u64),
        queries: Vec<surrealguard_embed::EmbeddedQuery>,
        host_source: &SourceId,
    ) -> HostCache {
        let source_id =
            |index: usize| SourceId::new(format!("embedded://{}#{index}", uri.as_str()));
        let schema_effecting = queries.iter().any(|query| is_schema_relevant(&query.text));

        let (analyzed, surql_texts) = if schema_effecting {
            self.analyze_host_queries_fully(queries, &source_id)
        } else {
            self.analyze_host_queries_incrementally(queries, &source_id)
        };
        // The host's own entry, so a finding's primary span renders. The map
        // is copied by entry (URIs and pointers), never by text.
        let mut texts = BTreeMap::clone(&surql_texts);
        texts.insert(uri.to_string(), (uri.clone(), Arc::clone(&target.text)));

        let diagnostics = analyzed
            .iter()
            .flat_map(|entry| {
                entry.output.diagnostics.iter().map(|finding| {
                    respan_to_host(finding, &entry.query, &entry.source, host_source)
                })
            })
            .collect();

        HostCache {
            key,
            queries: analyzed,
            diagnostics,
            texts: Arc::new(texts),
        }
    }

    /// The fast path: every embedded query is a pure query, so each analyzes
    /// alone against the cached catalog. The `.surql` cache is consulted (and
    /// rebuilt if the schema moved) exactly once for the whole host file.
    fn analyze_host_queries_incrementally(
        &self,
        queries: Vec<surrealguard_embed::EmbeddedQuery>,
        source_id: &impl Fn(usize) -> SourceId,
    ) -> (Vec<HostQuery>, SourceTexts) {
        let require = self.config.diagnostics.require_suppression_reasons;

        let (analyzed, texts, parsed_count) = self.with_surql_cache(|cache| {
            let mut analyzed = Vec::with_capacity(queries.len());
            let mut parsed_count = 0u64;
            for (index, query) in queries.into_iter().enumerate() {
                let source = source_id(index);
                let Ok(parsed) = parse_source(source.clone(), query.text.as_str()) else {
                    continue;
                };
                let output = analyze_one_source(&cache.catalog, &parsed, require);
                parsed_count += 1;
                analyzed.push(HostQuery {
                    query,
                    source,
                    output: Arc::new(output),
                    parsed: Some(Arc::new(parsed)),
                });
            }
            (analyzed, cache.texts(), parsed_count)
        });

        self.incremental_runs
            .fetch_add(parsed_count, Ordering::Relaxed);
        self.reanalyzed_sources
            .fetch_add(parsed_count, Ordering::Relaxed);
        (analyzed, texts)
    }

    /// The fallback: an embedded query carries a schema effect the catalog
    /// models (a write, a `DEFINE`), so it has to be analyzed *with* the
    /// workspace rather than against a snapshot of it.
    fn analyze_host_queries_fully(
        &self,
        queries: Vec<surrealguard_embed::EmbeddedQuery>,
        source_id: &impl Fn(usize) -> SourceId,
    ) -> (Vec<HostQuery>, SourceTexts) {
        let documents = self.surql_documents();
        let (mut analysis_workspace, surql_sources) = self.build_surql_workspace(&documents);
        let texts: SourceTexts = Arc::new(
            surql_sources
                .iter()
                .map(|(id, doc_uri, text)| (id.to_string(), (doc_uri.clone(), Arc::clone(text))))
                .collect(),
        );

        let registered: Vec<_> = queries
            .into_iter()
            .enumerate()
            .map(|(index, query)| {
                let source = analysis_workspace
                    .add_virtual_source(source_id(index).to_string(), query.text.clone());
                (source, query)
            })
            .collect();

        let mut workspace_output = analyze_workspace(&analysis_workspace);
        self.analyze_runs.fetch_add(1, Ordering::Relaxed);

        let analyzed = registered
            .into_iter()
            .filter_map(|(source, query)| {
                let output = workspace_output.sources.remove(&source)?;
                // The whole-workspace pass parsed the query internally and
                // kept nothing; one parse here, at cache-build time, is what
                // saves one per hover for as long as the entry lives.
                let parsed = parse_source(source.clone(), query.text.as_str())
                    .ok()
                    .map(Arc::new);
                Some(HostQuery {
                    query,
                    source,
                    output: Arc::new(output),
                    parsed,
                })
            })
            .collect();
        (analyzed, texts)
    }

    /// Analyze every tracked document — `.surql` and host alike — and return
    /// each one's findings keyed by URI. Runs the shared whole-workspace pass
    /// at most once (through the cache). Each result shares the document
    /// texts by `Arc`, so the whole list costs pointers and findings, not a
    /// copy of the workspace per document.
    ///
    /// The server itself publishes one document at a time (see
    /// [`Self::tracked_uris`]); this is the same sequence collected, for
    /// callers that want the whole picture at once.
    ///
    /// Host documents belong here for the same reason `.surql` ones do: a
    /// query embedded in a `.svelte` file reads the schema, so when the schema
    /// is saved its findings are as stale as any query file's.
    pub fn analyze_all(&self) -> Vec<(Url, DiagnosticAnalysisResult)> {
        self.tracked_uris()
            .into_iter()
            .filter_map(|uri| {
                let result = self.diagnostic_analysis(&uri)?;
                Some((uri, result))
            })
            .collect()
    }

    /// The analysis answering a cursor-addressed request inside a host file:
    /// the embedded query under `offset`, its own analysis output, and the map
    /// back to host coordinates. `None` when the cursor is outside every
    /// embedded query — including inside a `${...}` substitution, which names
    /// no position in the query the analyzer saw.
    pub fn host_feature_analysis(&self, uri: &Url, offset: usize) -> Option<HostFeatureAnalysis> {
        let target = self.documents.get(uri)?;
        if is_surrealql_uri(uri) || target.text.trim().is_empty() {
            return None;
        }
        let host = self.host_analysis(uri, target)?;
        let entry = host
            .queries
            .iter()
            .find(|entry| entry.query.host_range.contains(&offset))?;
        let embedded_offset = entry.query.embed_offset(offset)?;

        Some(HostFeatureAnalysis {
            output: Arc::clone(&entry.output),
            schema: self.with_surql_cache(|cache| Arc::clone(&cache.schema)),
            source: entry.source.clone(),
            text: entry.query.text.clone(),
            offset: embedded_offset,
            query: entry.query.clone(),
            parsed: entry.parsed.clone(),
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

        let text = Arc::clone(&target.text);
        let index = target.line_index();
        self.with_surql_cache(|cache| {
            let target_source = cache.source_for(uri)?.clone();
            let output = Arc::clone(cache.outputs.get(&target_source)?);
            // Absent when the document did not parse; a request then answers
            // from the analysis facts alone, exactly as a fresh parse attempt
            // would have. The text guard mirrors `completion_analysis`.
            let parsed = cache
                .parsed
                .get(&target_source)
                .filter(|parsed| parsed.text() == &*text)
                .cloned();
            Some(FeatureAnalysis {
                output,
                schema: Arc::clone(&cache.schema),
                source: target_source,
                text,
                index: Arc::clone(&index),
                sources: cache.texts(),
                parsed,
            })
        })
    }

    /// The inputs completion needs for a `.surql` document: the document's
    /// analysis output, the shared schema, and the **cached** parse tree.
    ///
    /// Completion fires on every keystroke, so this deliberately reuses the
    /// parse the analysis cache already holds rather than re-parsing, and — as
    /// with every other feature method — it reads the memoized analysis
    /// instead of running one. A completion request at an unchanged document
    /// state moves none of the analysis counters.
    pub fn completion_analysis(&self, uri: &Url) -> Option<CompletionAnalysis> {
        let target = self.documents.get(uri)?;
        if !is_surrealql_uri(uri) {
            return None;
        }
        let text = Arc::clone(&target.text);
        let index = target.line_index();
        self.with_surql_cache(|cache| {
            let target_source = cache.source_for(uri)?;
            let output = Arc::clone(cache.outputs.get(target_source)?);
            let parsed = cache.parsed.get(target_source)?.clone();
            // A parse that predates the current text would place candidates at
            // stale offsets; the cache key covers every document's text, so
            // this only guards against a future refactor breaking that.
            (parsed.text() == &*text).then(|| CompletionAnalysis {
                output,
                schema: Arc::clone(&cache.schema),
                parsed,
                text,
                index: Arc::clone(&index),
            })
        })
    }

    /// A `.surql` document's text and its **cached** parse tree — everything a
    /// syntax-only surface (semantic tokens) reads, without running or
    /// touching any analysis.
    pub fn parsed_surql(&self, uri: &Url) -> Option<(Arc<str>, Arc<ParsedSource>)> {
        let target = self.documents.get(uri)?;
        if !is_surrealql_uri(uri) {
            return None;
        }
        let text = Arc::clone(&target.text);
        self.with_surql_cache(|cache| {
            let parsed = cache.parsed.get(cache.source_for(uri)?)?.clone();
            (parsed.text() == &*text).then_some((text, parsed))
        })
    }

    /// A host document's text and the queries embedded in it. Extraction only:
    /// no schema, no analysis, no cache — what a query *is* does not depend on
    /// what the workspace knows about it.
    pub fn host_queries(
        &self,
        uri: &Url,
    ) -> Option<(Arc<str>, Vec<surrealguard_embed::EmbeddedQuery>)> {
        let target = self.documents.get(uri)?;
        if !is_host_uri(uri) {
            return None;
        }
        Some((
            Arc::clone(&target.text),
            surrealguard_embed::extract(uri.path(), &target.text),
        ))
    }

    /// Scan workspace folders for `.surql` and `.surrealql` files and load
    /// them.
    ///
    /// Dependency, build, and VCS directories are never descended into: a
    /// `.surql` under `node_modules` is someone else's, and walking `.git` or
    /// `target` on every start is pure cost. When the workspace carries a
    /// `surrealguard.toml`, its `[sources]` globs decide what is loaded — the
    /// same globs `surrealguard check` reads, evaluated relative to each root
    /// — so the editor analyzes exactly the set CI does. A file the globs
    /// exclude can still be opened and analyzed on its own, it just does not
    /// enter the workspace-wide analysis unasked.
    pub fn scan_folders(&mut self, sources: Option<&SourceConfig>) {
        let selector = sources.map(SourceSelector::new);
        for root in self.roots.clone() {
            let walker = walkdir::WalkDir::new(&root)
                .into_iter()
                .filter_entry(|entry| !is_skipped_dir(entry));
            for entry in walker.filter_map(std::result::Result::ok) {
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                let is_surrealql = path
                    .extension()
                    .is_some_and(|extension| extension == "surql" || extension == "surrealql");
                if !is_surrealql {
                    continue;
                }
                if let Some(selector) = &selector {
                    if !selector.selects(&root, path) {
                        continue;
                    }
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

/// Directories a scan never descends into, whatever the config says.
const SKIPPED_DIRS: [&str; 6] = [
    ".git",
    "node_modules",
    "target",
    "dist",
    ".svelte-kit",
    ".next",
];

/// Whether a walk entry is one of the directories in [`SKIPPED_DIRS`]. The
/// root itself (depth 0) is never skipped, even if the user opened a folder
/// that happens to carry one of those names.
fn is_skipped_dir(entry: &walkdir::DirEntry) -> bool {
    entry.depth() > 0
        && entry.file_type().is_dir()
        && entry
            .file_name()
            .to_str()
            .is_some_and(|name| SKIPPED_DIRS.contains(&name))
}

/// The `[sources]` globs compiled once for a scan. Mirrors the CLI's rule: a
/// file is a source when it matches a `schema` or `queries` glob and no
/// `ignore` glob, all evaluated on the path relative to the root.
struct SourceSelector {
    include: Vec<glob::Pattern>,
    ignore: Vec<glob::Pattern>,
}

impl SourceSelector {
    fn new(sources: &SourceConfig) -> Self {
        let compile = |globs: &[String]| {
            globs
                .iter()
                .filter_map(|glob| glob::Pattern::new(glob).ok())
                .collect::<Vec<_>>()
        };
        let mut include = compile(&sources.schema);
        include.extend(compile(&sources.queries));
        Self {
            include,
            ignore: compile(&sources.ignore),
        }
    }

    fn selects(&self, root: &Path, path: &Path) -> bool {
        let relative = path.strip_prefix(root).unwrap_or(path);
        let matches = |patterns: &[glob::Pattern]| {
            patterns
                .iter()
                .any(|pattern| pattern.matches_path(relative))
        };
        !matches(&self.ignore) && matches(&self.include)
    }
}

/// Whether a document is SurrealQL itself, rather than a host file whose
/// queries are embedded in string literals.
pub fn is_surrealql_uri(uri: &Url) -> bool {
    let path = uri.path();
    path.ends_with(".surql") || path.ends_with(".surrealql")
}

/// Whether a document is a host file that may carry embedded SurrealQL — the
/// same extension set `surrealguard check`/`generate` discover, so what the
/// editor flags and what CI flags can never disagree.
fn is_host_uri(uri: &Url) -> bool {
    const EXTENSIONS: [&str; 7] = ["ts", "tsx", "js", "jsx", "svelte", "vue", "astro"];
    matches!(
        uri.path().rsplit_once('.'),
        Some((_, extension)) if EXTENSIONS.contains(&extension)
    )
}

/// Whether a document might contribute to the global catalog or the workspace
/// schema — i.e. contains any statement that a *pure query* file does not. Used
/// to decide catalog-cache invalidation: if a `.surql` document is NOT
/// schema-relevant, editing it can only change its own analysis, so the cached
/// catalog (and every other source's output) stays valid.
///
/// The check is conservative: it whole-word-matches the schema-affecting
/// keywords, over-reporting (e.g. a keyword inside a string literal marks the
/// file relevant → a full rebuild), but never under-reporting — every
/// DEFINE/REMOVE/ALTER and every implicit-table-creating write
/// (CREATE/UPSERT/INSERT/DELETE) leads with one of these keywords.
///
/// Evaluated once per document, when its text arrives (see [`Document`]); the
/// verdict is read from there on every rebuild.
fn is_schema_relevant(text: &str) -> bool {
    const KEYWORDS: [&str; 7] = [
        "define", "remove", "alter", "create", "upsert", "insert", "delete",
    ];
    let lower = text.to_ascii_lowercase();
    KEYWORDS
        .iter()
        .any(|keyword| contains_whole_word(&lower, keyword))
}

/// Whether `haystack` (already lowercased) contains `keyword` as a whole word,
/// so a field named `created_at` does not mark a file as `CREATE`-relevant.
fn contains_whole_word(haystack: &str, keyword: &str) -> bool {
    let bytes = haystack.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut from = 0;
    while let Some(pos) = haystack[from..].find(keyword) {
        let start = from + pos;
        let end = start + keyword.len();
        let before_ok = start == 0 || !is_ident(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_ident(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

/// Rebuilds a finding computed on an embedded query so its primary span
/// points into the host file. Related spans (schema declarations) stay
/// where they are.
fn respan_to_host(
    finding: &Finding,
    query: &surrealguard_embed::EmbeddedQuery,
    embed_source: &surrealguard_syntax::source::SourceId,
    host_source: &surrealguard_syntax::source::SourceId,
) -> Finding {
    finding.map_spans(|span| {
        // A related span may already point into a `.surql` schema file (the
        // violated declaration); only spans inside the embedded query move.
        if span.source() != embed_source {
            return span.clone();
        }
        let range = span.range();
        let host = query.host_span(range.start() as usize..range.end() as usize);
        SourceSpan::new(
            host_source.clone(),
            ByteRange::new(host.start as u32, host.end as u32).expect("host spans are ordered"),
        )
    })
}

/// Full per-document analysis for editor features, carrying everything
/// the inlay-hint and hover handlers need to resolve types by span. Every
/// field is shared with the cache: building one is a handful of `Arc`
/// clones.
pub struct FeatureAnalysis {
    /// The target document's analysis output (statements, params).
    pub output: Arc<AnalysisOutput>,
    /// The schema shared across all `.surql` documents in the workspace.
    pub schema: Arc<SchemaIndex>,
    /// The target document's analysis source id.
    pub source: SourceId,
    /// The target document's full text, for offset/position conversion.
    pub text: Arc<str>,
    /// The line index over `text`, shared with the document so a request that
    /// converts many spans (inlay hints) pays for one index, not one scan per
    /// hint.
    pub index: Arc<LineIndex>,
    /// Every tracked `.surql` document keyed by its analysis source id
    /// (stringified), for resolving a definition span that points into another
    /// file back to its URI and text.
    pub sources: SourceTexts,
    /// The target document's cached parse tree, so a request walks it rather
    /// than re-parsing the text. `None` when the document did not parse.
    pub parsed: Option<Arc<ParsedSource>>,
}

/// Everything a cursor-addressed request needs to answer *inside* an embedded
/// query: the query's own analysis in embedded coordinates, plus the mapping
/// that puts the answer back on the host file.
pub struct HostFeatureAnalysis {
    /// The embedded query's analysis output.
    pub output: Arc<AnalysisOutput>,
    /// The schema shared across all `.surql` documents in the workspace.
    pub schema: Arc<SchemaIndex>,
    /// The embedded query's virtual source id.
    pub source: SourceId,
    /// The **embedded query's** text, not the host's — the coordinate system
    /// `output` and `offset` are expressed in.
    pub text: String,
    /// The requested host offset, translated into the query text.
    pub offset: usize,
    /// The extraction, for mapping a resulting span back to the host file.
    pub query: surrealguard_embed::EmbeddedQuery,
    /// The embedded query's cached parse tree. `None` when it did not parse.
    pub parsed: Option<Arc<ParsedSource>>,
}

/// Everything a completion request reads, all of it served from the cache.
pub struct CompletionAnalysis {
    /// The target document's analysis output.
    pub output: Arc<AnalysisOutput>,
    /// The schema shared across all `.surql` documents.
    pub schema: Arc<SchemaIndex>,
    /// The target document's cached parse tree.
    pub parsed: Arc<ParsedSource>,
    /// The target document's full text, for position/offset conversion.
    pub text: Arc<str>,
    /// The line index over `text`, shared with the document.
    pub index: Arc<LineIndex>,
}

/// Diagnostics-only result from the shared workspace analysis facade.
pub struct DiagnosticAnalysisResult {
    /// Findings for the target document, spanned into its own file.
    pub diagnostics: Vec<Finding>,
    /// The target document's full text, for rendering diagnostics.
    pub source: Arc<str>,
    /// The line index over `source`, shared with the document: a file's
    /// findings all convert against this one index rather than rescanning the
    /// text per finding.
    pub index: Arc<LineIndex>,
    /// Every analyzed document keyed by its analysis source id, for
    /// resolving related-information spans that point at other files.
    pub texts: SourceTexts,
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

        assert_eq!(&*analysis.source, "SELECT * FROM ;");
        assert_eq!(analysis.diagnostics.len(), 1);
        // `FROM ;` is a *missing* table name, which the parser reports as an
        // inserted node (S0002), not skipped input (S0001).
        assert_eq!(analysis.diagnostics[0].code().to_string(), "S0002");
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
    fn completion_reads_the_cache_and_never_analyzes() {
        let (workspace, _schema, query) = workspace_with_schema_and_query();

        // Warm the cache the way an editor session would: one edit, one
        // diagnostics publish.
        let _ = workspace.diagnostic_analysis(&query).expect("diagnostics");
        let counters = (
            workspace.analyze_run_count(),
            workspace.incremental_run_count(),
            workspace.reanalyzed_source_count(),
        );

        // A burst of completion requests — one per keystroke position —
        // must move none of the analysis counters.
        let text = "SELECT name FROM person;";
        for offset in 0..=text.len() as u32 {
            let analysis = workspace
                .completion_analysis(&query)
                .expect("completion inputs are served from the cache");
            let _ = surrealguard_workspace::complete_at(
                &analysis.output,
                &analysis.schema,
                &analysis.parsed,
                offset,
            );
        }

        assert_eq!(
            (
                workspace.analyze_run_count(),
                workspace.incremental_run_count(),
                workspace.reanalyzed_source_count(),
            ),
            counters,
            "completion must be a pure read of the memoized analysis"
        );
    }

    #[test]
    fn completion_reuses_the_cached_parse_rather_than_re_parsing() {
        let (workspace, _schema, query) = workspace_with_schema_and_query();
        let first = workspace.completion_analysis(&query).expect("inputs");
        let second = workspace.completion_analysis(&query).expect("inputs");
        // The same `Arc`, so no request re-parses the document.
        assert!(Arc::ptr_eq(&first.parsed, &second.parsed));
        assert_eq!(first.parsed.text(), "SELECT name FROM person;");
    }

    #[test]
    fn editing_a_query_document_takes_the_incremental_fast_path() {
        let (mut workspace, _schema, query) = workspace_with_schema_and_query();

        let _ = workspace.feature_analysis(&query).expect("features");
        assert_eq!(workspace.analyze_run_count(), 1);
        assert_eq!(workspace.incremental_run_count(), 0);

        // Editing the pure query document leaves the schema-defining inputs
        // unchanged -> the cached GlobalCatalog is reused and only the query is
        // re-analyzed. No new full workspace pass runs.
        workspace.upsert(
            query.clone(),
            "SELECT name FROM person WHERE name != NONE;".into(),
        );
        let _ = workspace
            .diagnostic_analysis(&query)
            .expect("re-analyzed after edit");
        assert_eq!(
            workspace.analyze_run_count(),
            1,
            "a query-only edit does NOT rebuild the whole workspace"
        );
        assert_eq!(
            workspace.incremental_run_count(),
            1,
            "a query-only edit re-analyzes exactly the dirty source"
        );

        // Re-upserting identical text keeps the same hash -> cache hit, no work.
        workspace.upsert(
            query.clone(),
            "SELECT name FROM person WHERE name != NONE;".into(),
        );
        let _ = workspace.feature_analysis(&query).expect("features");
        assert_eq!(workspace.analyze_run_count(), 1);
        assert_eq!(
            workspace.incremental_run_count(),
            1,
            "re-upserting identical text does not recompute"
        );
    }

    #[test]
    fn editing_a_schema_document_takes_the_symbol_incremental_path() {
        let (mut workspace, schema, query) = workspace_with_schema_and_query();

        let _ = workspace.feature_analysis(&query).expect("features");
        assert_eq!(workspace.analyze_run_count(), 1);
        assert_eq!(workspace.incremental_run_count(), 0);
        assert_eq!(workspace.reanalyzed_source_count(), 0);

        // Editing a schema-defining document adds a field to `person`. The
        // symbol diff sees `person`'s field set change, and the query reads
        // `person`, so BOTH the edited schema doc and the query re-analyze
        // through the symbol-incremental path — no full workspace pass runs.
        workspace.upsert(
            schema.clone(),
            "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD name ON person TYPE string;\n\
             DEFINE FIELD age ON person TYPE int;"
                .into(),
        );
        let _ = workspace
            .diagnostic_analysis(&query)
            .expect("re-analyzed after schema edit");
        assert_eq!(
            workspace.analyze_run_count(),
            1,
            "a schema edit no longer forces a full recompute"
        );
        assert_eq!(
            workspace.reanalyzed_source_count(),
            2,
            "the edited schema doc and the query that reads `person` re-analyze"
        );
    }

    #[test]
    fn schema_edit_reanalyzes_only_dependent_sources() {
        // Granularity: on a multi-file workspace, editing a function body that
        // nothing references re-analyzes exactly ONE source — the edited file —
        // where a full pass would re-analyze all N.
        let mut workspace = Workspace::new();
        let helper = Url::parse("file:///workspace/helper.surql").expect("uri");
        workspace.upsert(
            helper.clone(),
            "DEFINE FUNCTION fn::helper() -> int { RETURN 1; };".into(),
        );
        let mut count = 1usize;
        for i in 0..30 {
            let uri = Url::parse(&format!("file:///workspace/q{i}.surql")).expect("uri");
            workspace.upsert(uri, format!("RETURN {i};"));
            count += 1;
        }

        // Warm the cache with one full pass.
        let _ = workspace.analyze_all();
        assert_eq!(workspace.analyze_run_count(), 1);
        let before = workspace.reanalyzed_source_count();

        // Edit the helper's body without touching its signature. `fn::helper`
        // is referenced by no other source, so the symbol diff finds nothing
        // changed and only the edited file re-analyzes.
        workspace.upsert(
            helper.clone(),
            "DEFINE FUNCTION fn::helper() -> int { RETURN 2; };".into(),
        );
        let _ = workspace
            .diagnostic_analysis(&helper)
            .expect("re-analyzed after schema edit");
        assert_eq!(
            workspace.analyze_run_count(),
            1,
            "a symbol-incremental schema edit runs no full workspace pass"
        );
        assert_eq!(
            workspace.reanalyzed_source_count() - before,
            1,
            "only the edited source re-analyzes; the full path would do {count}"
        );
    }

    #[test]
    fn incremental_and_full_diagnostics_agree_for_a_schema_edit() {
        // End-to-end correctness for the schema path: adding the field a query
        // reads must clear that query's unknown-field finding incrementally,
        // matching a from-scratch pass at the edited state.
        let mut workspace = Workspace::new();
        let schema = Url::parse("file:///workspace/schema.surql").expect("uri");
        let query = Url::parse("file:///workspace/query.surql").expect("uri");
        workspace.upsert(
            schema.clone(),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;".into(),
        );
        workspace.upsert(query.clone(), "SELECT nickname FROM person;".into());
        let _ = workspace.analyze_all();

        let edited_schema = "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD name ON person TYPE string;\n\
             DEFINE FIELD nickname ON person TYPE string;";
        workspace.upsert(schema.clone(), edited_schema.into());
        let incremental = workspace
            .diagnostic_analysis(&query)
            .expect("incremental analysis after schema edit");
        assert_eq!(
            workspace.analyze_run_count(),
            1,
            "the schema edit took the symbol-incremental path"
        );

        // A pristine workspace at the edited state -> a guaranteed full pass.
        let mut fresh = Workspace::new();
        fresh.upsert(schema.clone(), edited_schema.into());
        fresh.upsert(query.clone(), "SELECT nickname FROM person;".into());
        let full = fresh.diagnostic_analysis(&query).expect("full analysis");

        let codes = |result: &DiagnosticAnalysisResult| {
            result
                .diagnostics
                .iter()
                .map(|f| {
                    (
                        f.code().to_string(),
                        f.span().range().start(),
                        f.span().range().end(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            codes(&incremental),
            codes(&full),
            "incremental schema-edit diagnostics must match a full pass"
        );
    }

    #[test]
    fn incremental_and_full_diagnostics_agree_for_a_query_edit() {
        // The correctness guarantee end to end: the fast path's per-source
        // diagnostics must equal what a from-scratch workspace produces for the
        // same edited state.
        let (mut workspace, _schema, query) = workspace_with_schema_and_query();
        let _ = workspace.analyze_all();

        let edited = "SELECT name, missing FROM person WHERE age > $min;";
        workspace.upsert(query.clone(), edited.into());
        let incremental = workspace
            .diagnostic_analysis(&query)
            .expect("incremental analysis");

        // A pristine workspace at the edited state -> a guaranteed full pass.
        let mut fresh = Workspace::new();
        fresh.upsert(
            Url::parse("file:///workspace/schema.surql").expect("uri"),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;".into(),
        );
        fresh.upsert(query.clone(), edited.into());
        let full = fresh.diagnostic_analysis(&query).expect("full analysis");

        let codes = |result: &DiagnosticAnalysisResult| {
            result
                .diagnostics
                .iter()
                .map(|f| {
                    (
                        f.code().to_string(),
                        f.span().range().start(),
                        f.span().range().end(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            codes(&incremental),
            codes(&full),
            "incremental diagnostics must match a full pass at the same state"
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
            "const q = db.query(\"SELECT name FROM person\");".into(),
        );

        // Warm the shared `.surql` schema pass so the counts below reflect
        // only host recomputes. `analyze_all` now covers the host document
        // too, so the first host analysis has already happened here.
        let _ = workspace.analyze_all();
        let full = workspace.analyze_run_count();
        let reanalyzed = workspace.reanalyzed_source_count();
        assert_eq!(
            reanalyzed, 1,
            "the host's one embedded query analyzed exactly once"
        );

        // Same host + schema state -> cache hit, no work at all.
        let _ = workspace.diagnostic_analysis(&host).expect("host reused");
        assert_eq!(
            (
                workspace.analyze_run_count(),
                workspace.reanalyzed_source_count()
            ),
            (full, reanalyzed),
            "unchanged host state reuses the cached analysis"
        );

        // Editing the host text invalidates its entry — and costs exactly the
        // one embedded query, not a whole-workspace pass.
        workspace.upsert(
            host.clone(),
            "const q = db.query(\"SELECT name FROM person WHERE name != NONE\");".into(),
        );
        let _ = workspace
            .diagnostic_analysis(&host)
            .expect("host re-analyzed");
        assert_eq!(
            workspace.analyze_run_count(),
            full,
            "a host edit runs no full workspace pass"
        );
        assert_eq!(
            workspace.reanalyzed_source_count(),
            reanalyzed + 1,
            "a host edit re-analyzes exactly its embedded queries"
        );
    }

    #[test]
    fn a_host_file_with_no_embedded_query_costs_nothing() {
        let (mut workspace, _schema, _query) = workspace_with_schema_and_query();
        let _ = workspace.analyze_all();
        let counters = (
            workspace.analyze_run_count(),
            workspace.incremental_run_count(),
            workspace.reanalyzed_source_count(),
        );

        let host = Url::parse("file:///workspace/plain.ts").expect("valid uri");
        workspace.upsert(host.clone(), "export const answer = 42;\n".into());
        let result = workspace
            .diagnostic_analysis(&host)
            .expect("a host file is still a tracked document");

        assert!(
            result.diagnostics.is_empty(),
            "a host file with no embedded query says nothing"
        );
        assert_eq!(
            (
                workspace.analyze_run_count(),
                workspace.incremental_run_count(),
                workspace.reanalyzed_source_count(),
            ),
            counters,
            "and analyzes nothing to say it"
        );
    }

    #[test]
    fn a_document_we_do_not_understand_is_not_a_host_file() {
        // The client may attach us to more than we can read. Parsing a
        // lockfile as TypeScript on every keystroke is pure cost, so those
        // documents are not analyzed at all.
        let mut workspace = Workspace::new();
        let other = Url::parse("file:///workspace/package-lock.json").expect("valid uri");
        workspace.upsert(other.clone(), "{ \"name\": \"surql\" }\n".into());
        assert!(workspace.diagnostic_analysis(&other).is_none());
    }

    #[test]
    fn host_diagnostics_follow_a_schema_edit() {
        // The reason host documents belong in `analyze_all`: a query embedded
        // in a `.svelte` file reads the schema, so a schema edit makes its
        // findings as stale as any query file's.
        let mut workspace = Workspace::new();
        let schema = Url::parse("file:///workspace/schema.surql").expect("valid uri");
        workspace.upsert(
            schema.clone(),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;".into(),
        );
        let host = Url::parse("file:///workspace/app.svelte").expect("valid uri");
        workspace.upsert(
            host.clone(),
            "<script>const q = db.query(\"SELECT nickname FROM person\");</script>".into(),
        );

        // Errors only: the lint about reading a whole table is a matter of
        // taste and fires either way.
        let errors = |results: &[(Url, DiagnosticAnalysisResult)], uri: &Url| {
            results
                .iter()
                .find(|(result_uri, _)| result_uri == uri)
                .map(|(_, result)| {
                    result
                        .diagnostics
                        .iter()
                        .filter(|finding| {
                            finding.severity() == surrealguard_diagnostics::Severity::Error
                        })
                        .map(|finding| finding.message().to_string())
                        .collect::<Vec<_>>()
                })
        };

        let before = errors(&workspace.analyze_all(), &host);
        assert_eq!(
            before,
            Some(vec!["`person` has no field `nickname`".to_string()]),
            "the embedded query is checked against the schema"
        );

        workspace.upsert(
            schema,
            "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD name ON person TYPE string;\n\
             DEFINE FIELD nickname ON person TYPE string;"
                .into(),
        );
        assert_eq!(
            errors(&workspace.analyze_all(), &host),
            Some(Vec::new()),
            "defining the field clears the host file's finding"
        );
    }

    /// A throwaway on-disk workspace: `schema/a.surql`, `queries/b.surql`, a
    /// stray `notes/c.surql`, and a `.surql` inside each directory a scan must
    /// never enter. Removed on drop.
    struct ScratchRoot(PathBuf);

    impl ScratchRoot {
        fn create(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "surrealguard-lsp-scan-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            for (relative, text) in [
                ("schema/a.surql", "DEFINE TABLE person SCHEMAFULL;"),
                ("queries/b.surql", "SELECT * FROM person;"),
                ("notes/c.surql", "SELECT * FROM person;"),
                ("node_modules/dep/d.surql", "SELECT * FROM person;"),
                ("target/e.surql", "SELECT * FROM person;"),
                (".git/f.surql", "SELECT * FROM person;"),
                (".svelte-kit/g.surql", "SELECT * FROM person;"),
            ] {
                let path = root.join(relative);
                std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
                std::fs::write(path, text).expect("write fixture");
            }
            Self(root)
        }

        fn scanned(&self, sources: Option<&SourceConfig>) -> Vec<String> {
            let mut workspace = Workspace::new();
            workspace.roots = vec![self.0.clone()];
            workspace.scan_folders(sources);
            let mut names: Vec<String> = workspace
                .documents()
                .map(|doc| {
                    let path = doc.uri.to_file_path().expect("file uri");
                    path.strip_prefix(&self.0)
                        .expect("under root")
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for ScratchRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_scan_never_enters_dependency_build_or_vcs_directories() {
        let root = ScratchRoot::create("skips");
        assert_eq!(
            root.scanned(None),
            vec!["notes/c.surql", "queries/b.surql", "schema/a.surql"],
            "without a config every .surql outside the skipped directories loads"
        );
    }

    #[test]
    fn a_scan_honours_the_source_globs_of_surrealguard_toml() {
        let root = ScratchRoot::create("globs");
        let config = WorkspaceConfig::from_toml_str(
            "[sources]\nschema = [\"schema/**/*.surql\"]\nqueries = [\"queries/**/*.surql\"]\n",
        )
        .expect("valid config");
        assert_eq!(
            root.scanned(Some(&config.sources)),
            vec!["queries/b.surql", "schema/a.surql"],
            "only what the globs select enters the workspace analysis"
        );

        // `ignore` wins over an include, exactly as in `surrealguard check`.
        let config = WorkspaceConfig::from_toml_str(
            "[sources]\nschema = [\"**/*.surql\"]\nqueries = [\"**/*.surql\"]\nignore = [\"notes/**\"]\n",
        )
        .expect("valid config");
        assert_eq!(
            root.scanned(Some(&config.sources)),
            vec!["queries/b.surql", "schema/a.surql"]
        );
    }

    #[test]
    fn a_host_cursor_resolves_through_the_embedded_query() {
        let mut workspace = Workspace::new();
        let schema = Url::parse("file:///workspace/schema.surql").expect("valid uri");
        workspace.upsert(
            schema,
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;".into(),
        );
        let host = Url::parse("file:///workspace/app.svelte").expect("valid uri");
        let text =
            "<script lang=\"ts\">\n  const q = db.query(\"SELECT name FROM person\");\n</script>\n";
        workspace.upsert(host.clone(), text.into());

        let at = text.find("person").expect("host offset of the table name");
        let analysis = workspace
            .host_feature_analysis(&host, at)
            .expect("the cursor is inside the embedded query");
        assert_eq!(
            &analysis.text[analysis.offset..analysis.offset + 6],
            "person",
            "the host offset names the same token in the query"
        );

        // A cursor in the surrounding host language is not ours to answer.
        let outside = text.find("const").expect("host code outside the template");
        assert!(workspace.host_feature_analysis(&host, outside).is_none());
    }
}
