//! LSP backend — implements the `LanguageServer` trait.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use surrealql_analyzer_diagnostics::PolicyConfig;
use surrealql_analyzer_syntax::parse::{parse_source, ParsedSource};
use surrealql_analyzer_syntax::source::SourceId;
use surrealql_analyzer_workspace::config::WorkspaceConfig;
use surrealql_analyzer_workspace::query::{
    definition_at_lowered, definition_at_parsed, function_return_hints_parsed, hover_at_lowered,
    hover_at_parsed, DefinitionTarget, HoverInfo,
};
use surrealql_analyzer_workspace::{AnalysisOutput, SchemaIndex};
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::text::LineIndex;
use crate::workspace::Workspace;
use crate::{code_action, completion, diagnostics, semantic};

/// The `surrealql-analyzer.toml` behind the workspace policy: where it is, and the
/// text last successfully parsed from it. Kept so the "suppress workspace-wide"
/// action can edit the real file, and so a change to it can be noticed.
#[derive(Clone, Debug)]
struct LoadedConfig {
    path: PathBuf,
    text: String,
    config: WorkspaceConfig,
}

/// The language server: holds the LSP client handle, the tracked workspace,
/// and the severity policy resolved from `surrealql-analyzer.toml`. Implements
/// [`tower_lsp::LanguageServer`].
pub struct Backend {
    client: Client,
    workspace: RwLock<Workspace>,
    /// Severity policy from `surrealql-analyzer.toml`, so `[lints]` levels apply in
    /// the editor exactly as they do in `surrealql-analyzer check`. Defaults until
    /// `initialize` locates a config in a workspace root.
    policy: RwLock<PolicyConfig>,
    /// State of the one `workspace/semanticTokens/refresh` request that may
    /// be outstanding at a time; see [`Self::refresh_semantic_tokens`].
    /// Shared with the task that sends it.
    refresh: Arc<RefreshState>,
    /// The last semantic-token answer per document, keyed by the text it was
    /// computed from; see [`Self::semantic_tokens_full`].
    semantic_cache: Mutex<HashMap<Url, SemanticEntry>>,
    /// The `surrealql-analyzer.toml` the policy came from, when a workspace root has
    /// one. `None` leaves the workspace-wide suppression action unoffered:
    /// creating a config file is a resource operation not every client
    /// supports, and inventing one behind the user's back is not a quick fix.
    config: RwLock<Option<LoadedConfig>>,
    /// Whether the client declared it can receive `CodeAction` literals. A
    /// client that only understands `Command` would get objects it never asked
    /// for, so with this false the server advertises no code-action provider
    /// and answers `textDocument/codeAction` with nothing.
    code_action_literal_support: AtomicBool,
    /// Whether the client can be asked to watch files for us. The
    /// "suppress workspace-wide" action edits `surrealql-analyzer.toml` through the
    /// client, which then tells us nothing about it; a watcher is how the
    /// allowed diagnostic disappears without waiting for the next keystroke.
    /// Registration is a server->client *request*, so — like semantic-token
    /// refresh — it is only sent to a client that declared it accepts one.
    watched_files_support: AtomicBool,
}

/// One document's encoded semantic tokens and the text they describe. The
/// text is held by `Arc`, so an unchanged document is recognized by pointer
/// and the entry can never outlive the allocation it points at.
struct SemanticEntry {
    text: Arc<str>,
    data: Arc<Vec<SemanticToken>>,
}

/// Hover over a source through its cached parse, or — when the source did
/// not parse — from the analysis facts alone (bindings, params, tables), which
/// is exactly what a fresh parse attempt would have fallen back to.
fn hover_from_cache(
    output: &AnalysisOutput,
    schema: &SchemaIndex,
    parsed: Option<&ParsedSource>,
    source: &SourceId,
    text: &str,
    offset: u32,
) -> Option<HoverInfo> {
    match parsed {
        Some(parsed) => hover_at_parsed(output, schema, parsed, offset),
        None => hover_at_lowered(output, schema, source, text, &[], offset),
    }
}

/// Go-to-definition through the cached parse; an unparsed source still
/// resolves `LET` binding sites, which are keyed on the analysis output.
fn definition_from_cache(
    output: &AnalysisOutput,
    schema: &SchemaIndex,
    parsed: Option<&ParsedSource>,
    source: &SourceId,
    offset: u32,
) -> Option<DefinitionTarget> {
    match parsed {
        Some(parsed) => definition_at_parsed(output, schema, parsed, offset),
        None => definition_at_lowered(output, schema, source, &[], offset),
    }
}

/// Gate and coalescing state for `workspace/semanticTokens/refresh`.
#[derive(Debug, Default)]
struct RefreshState {
    /// Whether the client advertised `workspace.semanticTokens.refreshSupport`
    /// at `initialize`. Nothing is sent to a client that did not.
    supported: AtomicBool,
    /// Whether a refresh request is currently awaiting the client's answer.
    in_flight: AtomicBool,
    /// Whether a refresh was asked for while one was in flight, so the
    /// in-flight task sends one more when its answer lands.
    dirty: AtomicBool,
}

impl Backend {
    /// Builds a backend bound to the given LSP client, with an empty
    /// workspace and the default (no-config) policy.
    pub fn new(client: Client) -> Self {
        Self {
            client,
            workspace: RwLock::new(Workspace::new()),
            policy: RwLock::new(PolicyConfig::default()),
            refresh: Arc::new(RefreshState::default()),
            semantic_cache: Mutex::new(HashMap::new()),
            config: RwLock::new(None),
            code_action_literal_support: AtomicBool::new(false),
            watched_files_support: AtomicBool::new(false),
        }
    }

    /// Re-reads `surrealql-analyzer.toml` and rebuilds the policy when its text
    /// changed on disk.
    ///
    /// The editor is the one surface where the config can move *while the
    /// server runs* — the "suppress workspace-wide" action edits it, and the
    /// client applies that edit without telling us anything. Without this, an
    /// accepted action leaves the diagnostic it just allowed still squiggled
    /// until the next restart.
    ///
    /// A read per publish, of a file measured in hundreds of bytes, alongside
    /// an analysis pass. A config that fails to parse mid-edit leaves both the
    /// cached text and the policy alone, so the next publish retries rather
    /// than latching a half-written file.
    ///
    /// The config is also an analysis input (`[analysis]`, `[diagnostics]`),
    /// so the workspace is handed the new one too, which drops every cached
    /// analysis computed under the old.
    async fn reload_config(&self) {
        let Some(path) = ({ self.config.read().await.as_ref().map(|c| c.path.clone()) }) else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        if self
            .config
            .read()
            .await
            .as_ref()
            .is_some_and(|loaded| loaded.text == text)
        {
            return;
        }
        let Ok(config) = WorkspaceConfig::from_toml_str(&text) else {
            return;
        };
        *self.policy.write().await = config.policy();
        self.workspace.write().await.set_config(config.clone());
        *self.config.write().await = Some(LoadedConfig { path, text, config });
    }

    /// The cached semantic tokens for `uri` if they were computed from
    /// exactly `text` (same allocation, or a re-upsert of identical bytes);
    /// otherwise runs `compute`, caches its answer under `text`, and returns
    /// it. Returns an owned copy because the protocol type owns its data.
    fn semantic_tokens_for(
        &self,
        uri: &Url,
        text: &Arc<str>,
        compute: impl FnOnce() -> Vec<SemanticToken>,
    ) -> Vec<SemanticToken> {
        let mut cache = self
            .semantic_cache
            .lock()
            .expect("semantic token cache mutex poisoned");
        if let Some(entry) = cache.get(uri) {
            if Arc::ptr_eq(&entry.text, text) || *entry.text == **text {
                return entry.data.to_vec();
            }
        }
        let data = Arc::new(compute());
        cache.insert(
            uri.clone(),
            SemanticEntry {
                text: Arc::clone(text),
                data: Arc::clone(&data),
            },
        );
        data.to_vec()
    }

    /// Analyze one document and publish its diagnostics, then ask the client
    /// to re-pull semantic tokens. The shape of every single-document edit
    /// path (`didOpen`, `didChange`).
    async fn publish_diagnostics(&self, uri: &Url) {
        self.reload_config().await;
        self.publish_document_diagnostics(uri).await;
        self.refresh_semantic_tokens();
    }

    /// Analyze one document and publish its diagnostics — nothing else. The
    /// analysis result shares the workspace's text by `Arc`, so this costs
    /// the document's findings and no copy of anything.
    async fn publish_document_diagnostics(&self, uri: &Url) {
        let result = {
            let ws = self.workspace.read().await;
            ws.diagnostic_analysis(uri)
        };

        let Some(result) = result else {
            return;
        };

        // Presentation policy (from surrealql-analyzer.toml) applies here, at the
        // consumption edge; the findings themselves carry only their
        // intrinsic class.
        let policy = self.policy.read().await;
        // One context for the whole document: every finding converts against
        // the index the document already holds.
        let context = diagnostics::DiagnosticContext::new(&result.index, &result.texts);
        let lsp_diagnostics: Vec<Diagnostic> = result
            .diagnostics
            .iter()
            .filter_map(|d| diagnostics::workspace_finding_to_lsp_diagnostic(&context, d, &policy))
            .collect();
        drop(policy);

        self.client
            .publish_diagnostics(uri.clone(), lsp_diagnostics, None)
            .await;
    }

    /// Ask the client to re-request semantic tokens.
    ///
    /// Diagnostics are *pushed*; semantic tokens are *pulled* — the client asks
    /// once, caches the answer, and re-asks only when told to. Without this, our
    /// tokens are whatever we said the first time the buffer was opened, and any
    /// later invalidation leaves them stale.
    ///
    /// That is visible, not theoretical. In a `.svelte` buffer Zed keeps every
    /// server's tokens separately and lets later ones win on overlap; the Svelte
    /// server *does* refresh, so regenerating the query registry (which
    /// invalidates its TypeScript project) got it a fresh answer while ours went
    /// stale — and the highlighted query reverted to plain-string green exactly
    /// when the generated types were rewritten.
    ///
    /// The request is workspace-wide because that is the only granularity the
    /// protocol offers, and it is cheap: it makes the client re-ask, and our
    /// answer for an unchanged document comes from the analysis cache.
    ///
    /// It is sent only to a client that advertised
    /// `workspace.semanticTokens.refreshSupport`. A client without it does not
    /// merely answer with an error: some never answer at all, and this is a
    /// server→client *request*, so every unanswered one sits in tower-lsp's
    /// pending-response table forever — about a kilobyte per keystroke that
    /// was never freed.
    ///
    /// It is also coalesced: at most one refresh is outstanding. A request
    /// while one is in flight marks it dirty, and the in-flight task sends one
    /// more when its answer lands — so a burst of keystrokes costs one round
    /// trip, and a client that never answers holds exactly one pending entry
    /// rather than one per edit. Nothing is lost by folding: a refresh carries
    /// no payload, it only says "ask again".
    ///
    /// It is spawned rather than awaited. Awaiting a server→client request
    /// blocks the handler until the client replies — and a client that never
    /// replies blocks it forever. That is not hypothetical: awaiting here
    /// deadlocked every `crates/lsp/tests/backend.rs` case that publishes
    /// diagnostics, because the harness drives the server directly and answers
    /// no requests. A real editor would have replied, so the bug would have
    /// reached a release looking like a hang under some other client.
    fn refresh_semantic_tokens(&self) {
        let state = &self.refresh;
        if !state.supported.load(Ordering::Acquire) {
            return;
        }
        // Record the wish first, then try to become the sender. If a sender
        // already exists it is guaranteed to observe `dirty` after its await
        // (or hand over below), so the wish is never lost.
        state.dirty.store(true, Ordering::SeqCst);
        if state.in_flight.swap(true, Ordering::SeqCst) {
            return;
        }

        let client = self.client.clone();
        let state = Arc::clone(state);
        tokio::spawn(async move {
            loop {
                state.dirty.store(false, Ordering::SeqCst);
                // Nothing about the document depends on the outcome, so a
                // failure is not worth surfacing to the user.
                let _ = client.semantic_tokens_refresh().await;

                if state.dirty.load(Ordering::SeqCst) {
                    continue;
                }
                state.in_flight.store(false, Ordering::SeqCst);
                // A request that arrived between the load and the store set
                // `dirty`, saw `in_flight`, and did not spawn. Pick it up here
                // — unless a newer request already re-took the slot, in which
                // case that one owns it now.
                if state.dirty.load(Ordering::SeqCst)
                    && !state.in_flight.swap(true, Ordering::SeqCst)
                {
                    continue;
                }
                break;
            }
        });
    }

    /// Asks the client to watch `surrealql-analyzer.toml`, when it said it would.
    ///
    /// Spawned, never awaited, for the same reason semantic-token refresh is:
    /// this is a server->client *request*, and awaiting one inside a handler
    /// blocks that handler until the client answers. Nothing here depends on
    /// the outcome — without the watcher a config change is still picked up on
    /// the next publish, just not instantly.
    fn watch_config_file(&self) {
        if !self.watched_files_support.load(Ordering::Relaxed) {
            return;
        }
        let Ok(options) = serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
            watchers: vec![FileSystemWatcher {
                glob_pattern: GlobPattern::String("**/surrealql-analyzer.toml".to_string()),
                kind: None,
            }],
        }) else {
            return;
        };
        let client = self.client.clone();
        tokio::spawn(async move {
            let _ = client
                .register_capability(vec![Registration {
                    id: "surrealql-analyzer-config-watcher".to_string(),
                    method: "workspace/didChangeWatchedFiles".to_string(),
                    register_options: Some(options),
                }])
                .await;
        });
    }

    /// Hover for a position inside a host file's embedded query: the cursor is
    /// translated into the query, answered by the query's own analysis, and the
    /// resulting span mapped back onto the host text — so a field inside a
    /// `` surql`…` `` template reports the same type it would in a `.surql`
    /// file. `None` for `.surql` documents and for a host position outside
    /// every embedded query, both of which the ordinary path handles.
    async fn host_hover(&self, uri: &Url, position: Position) -> Option<Hover> {
        let (index, analysis) = {
            let ws = self.workspace.read().await;
            let index = ws.line_index(uri)?;
            let offset = index.position_to_offset(position);
            (index, ws.host_feature_analysis(uri, offset)?)
        };

        let info = hover_from_cache(
            &analysis.output,
            &analysis.schema,
            analysis.parsed.as_deref(),
            &analysis.source,
            &analysis.text,
            analysis.offset as u32,
        )?;

        // The span is in embedded coordinates; the editor is looking at the
        // host file.
        let range = info.span.range();
        let host = analysis
            .query
            .host_span(range.start() as usize..range.end() as usize);
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: info.markdown,
            }),
            range: Some(index.range(host.start, host.end)),
        })
    }

    /// The surrealql-analyzer diagnostics a code-action request is about, each
    /// paired with the canonical code a suppression must name, deduplicated so
    /// one code never yields two identical actions.
    ///
    /// The client's `context.diagnostics` is the authority — it is what the
    /// user's cursor is actually on. Some clients send an empty context when
    /// the request comes from a keybinding rather than a lightbulb, so an
    /// empty one falls back to our own findings overlapping the request range.
    async fn suppressible_diagnostics(
        &self,
        uri: &Url,
        params: &CodeActionParams,
    ) -> Vec<(String, Diagnostic)> {
        let mut candidates: Vec<Diagnostic> = params
            .context
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.source.as_deref() == Some("surrealql-analyzer"))
            .cloned()
            .collect();
        if candidates.is_empty() {
            candidates = self.diagnostics_overlapping(uri, params.range).await;
        }

        let mut seen = std::collections::BTreeSet::new();
        candidates
            .into_iter()
            .filter_map(|diagnostic| {
                let Some(NumberOrString::String(rendered)) = &diagnostic.code else {
                    return None;
                };
                let code = code_action::suppressible_code(rendered)?;
                seen.insert(code.clone()).then_some((code, diagnostic))
            })
            .collect()
    }

    /// The diagnostics this server would publish for `uri` that touch `range`.
    async fn diagnostics_overlapping(&self, uri: &Url, range: Range) -> Vec<Diagnostic> {
        let result = {
            let ws = self.workspace.read().await;
            ws.diagnostic_analysis(uri)
        };
        let Some(result) = result else {
            return Vec::new();
        };
        let policy = self.policy.read().await;
        let context = diagnostics::DiagnosticContext::new(&result.index, &result.texts);
        result
            .diagnostics
            .iter()
            .filter_map(|finding| {
                diagnostics::workspace_finding_to_lsp_diagnostic(&context, finding, &policy)
            })
            .filter(|diagnostic| ranges_overlap(diagnostic.range, range))
            .collect()
    }

    /// Whether an inline directive inserted above `offset` would actually
    /// suppress — the only condition under which the action is offered.
    ///
    /// Two ways it would not. In a host file the query lives in a string
    /// literal, and only a multi-line backtick template can hold a comment
    /// line (see [`code_action::host_inline_site`]). And in *any* file,
    /// suppression is skipped entirely for a source that failed to parse, so a
    /// document with a syntax error would take the directive and keep the
    /// finding.
    async fn inline_suppression_possible(&self, uri: &Url, offset: usize) -> bool {
        let ws = self.workspace.read().await;

        if crate::workspace::is_surrealql_uri(uri) {
            return ws
                .parsed_surql(uri)
                .is_some_and(|(_, parsed)| parsed.syntax_diagnostics().is_empty());
        }

        let Some((host_text, queries)) = ws.host_queries(uri) else {
            return false;
        };
        if !code_action::host_inline_site(&host_text, &queries, offset) {
            return false;
        }
        queries
            .iter()
            .find(|query| query.host_range.contains(&offset))
            .and_then(|query| {
                parse_source(SourceId::new("code-action://probe"), query.text.as_str()).ok()
            })
            .is_some_and(|parsed| parsed.syntax_diagnostics().is_empty())
    }

    /// Publish diagnostics for every tracked document. The first `.surql`
    /// document warms the shared whole-workspace analysis once; every later
    /// one reads the cache. Documents are analyzed and published one at a
    /// time — never a list of every result at once — so the sweep holds one
    /// document's findings in flight, not the workspace's. On a 500-file
    /// workspace the old all-at-once list, each entry carrying its own copy
    /// of every text, was the difference between 90 MB and 900 MB resident.
    async fn publish_all_diagnostics(&self) {
        self.reload_config().await;
        let uris = {
            let ws = self.workspace.read().await;
            ws.tracked_uris()
        };
        for uri in &uris {
            self.publish_document_diagnostics(uri).await;
        }

        // One request for the whole sweep: this runs when the schema moved, so
        // every open document's tokens are suspect, and the protocol has no
        // per-document form anyway.
        self.refresh_semantic_tokens();
    }
}

/// Whether two LSP ranges share at least a boundary point.
fn ranges_overlap(left: Range, right: Range) -> bool {
    let key = |position: Position| (position.line, position.character);
    key(left.start) <= key(right.end) && key(right.start) <= key(left.end)
}

/// A quick fix that applies one edit to one file.
///
/// `changes` rather than `documentChanges`: the richer form is a separate
/// client capability, and a single edit to a single existing file needs
/// nothing it adds.
fn suppression_action(
    title: String,
    uri: Url,
    edit: TextEdit,
    diagnostic: Diagnostic,
) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diagnostic]),
        edit: Some(WorkspaceEdit {
            changes: Some([(uri, vec![edit])].into_iter().collect()),
            ..WorkspaceEdit::default()
        }),
        ..CodeAction::default()
    })
}

/// Loads and parses `surrealql-analyzer.toml` from a workspace root. Returns
/// `None` when the root has no config file; a malformed config is treated as
/// absent (the editor falls back to the default policy rather than failing to
/// start).
fn load_workspace_config(root: &Path) -> Option<LoadedConfig> {
    let path = root.join("surrealql-analyzer.toml");
    let text = std::fs::read_to_string(&path).ok()?;
    let config = WorkspaceConfig::from_toml_str(&text).ok()?;
    Some(LoadedConfig { path, text, config })
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Whether the client will honor `workspace/semanticTokens/refresh`.
        // Decided once, here: a refresh is never sent to a client that did
        // not ask for one (see `refresh_semantic_tokens`).
        let refresh_support = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.semantic_tokens.as_ref())
            .and_then(|tokens| tokens.refresh_support)
            .unwrap_or(false);
        self.refresh
            .supported
            .store(refresh_support, Ordering::Release);

        // Code actions are only offered to a client that said it can receive
        // them as literals. `codeActionLiteralSupport` is the flag that means
        // "send me `CodeAction` objects, not bare `Command`s"; without it the
        // only legal answer is a command the client would then have to execute
        // through `workspace/executeCommand`, which is a capability of its own.
        // Rather than send a shape nobody advertised, offer nothing at all.
        let code_action_literals = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|text| text.code_action.as_ref())
            .and_then(|action| action.code_action_literal_support.as_ref())
            .is_some();
        self.code_action_literal_support
            .store(code_action_literals, Ordering::Relaxed);

        let watched_files = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.did_change_watched_files.as_ref())
            .and_then(|watched| watched.dynamic_registration)
            .unwrap_or(false);
        self.watched_files_support
            .store(watched_files, Ordering::Relaxed);

        if let Some(folders) = &params.workspace_folders {
            let roots: Vec<_> = folders
                .iter()
                .filter_map(|f| f.uri.to_file_path().ok())
                .collect();

            // The first workspace root that carries a surrealql-analyzer.toml
            // speaks for the workspace: its `[lints]` levels become the
            // editor's policy and its `[sources]` globs decide what the scan
            // loads, so the editor honors the same config as
            // `surrealql-analyzer check`. No config leaves the defaults. The file
            // itself is remembered so the workspace-wide suppression action
            // can edit it, and so a later change to it is noticed.
            let loaded = roots.iter().find_map(|root| load_workspace_config(root));
            if let Some(loaded) = &loaded {
                *self.policy.write().await = loaded.config.policy();
                *self.config.write().await = Some(loaded.clone());
            }

            let mut ws = self.workspace.write().await;
            ws.roots = roots;
            if let Some(loaded) = loaded {
                // `[analysis]` (the target SurrealDB version behind the 8xxx
                // checks) and `[diagnostics]` are analysis inputs, not
                // presentation: they go into the analysis itself.
                ws.scan_folders(Some(&loaded.config.sources));
                ws.set_config(loaded.config);
            } else {
                ws.scan_folders(None);
            }
        }

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "surrealql-analyzer-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                // Highlighting inside a host file's template can only come
                // from here: the editor's grammar for a `.svelte` or `.ts`
                // file sees the query as one string, and the injection that
                // would fix that has to be declared by the host language, not
                // by us. Advertised for `.surql` too so both agree.
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: SemanticTokensLegend {
                                token_types: semantic::TOKEN_TYPES.to_vec(),
                                token_modifiers: Vec::new(),
                            },
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            range: Some(false),
                            ..SemanticTokensOptions::default()
                        },
                    ),
                ),
                inlay_hint_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                completion_provider: Some(CompletionOptions {
                    // `$` opens a parameter, `.` a member, `:` closes the
                    // `::` of a function path, and the rest are the clause
                    // boundaries where a fresh name starts. Identifier
                    // characters need no trigger: clients re-request as the
                    // word grows.
                    trigger_characters: Some(
                        ["$", ".", ":", " ", ",", "(", "{", ">"]
                            .map(str::to_string)
                            .to_vec(),
                    ),
                    // Items are complete as sent; nothing is resolved lazily.
                    resolve_provider: Some(false),
                    ..CompletionOptions::default()
                }),
                code_action_provider: code_action_literals.then(|| {
                    CodeActionProviderCapability::Options(CodeActionOptions {
                        // Only quick fixes: a suppression is offered against a
                        // specific diagnostic, never as a standalone refactor.
                        code_action_kinds: Some(vec![CodeActionKind::QUICKFIX]),
                        // Every action carries its complete edit when it is
                        // offered; there is nothing left to resolve.
                        resolve_provider: Some(false),
                        work_done_progress_options: WorkDoneProgressOptions::default(),
                    })
                }),
                ..ServerCapabilities::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.watch_config_file();
        self.publish_all_diagnostics().await;
        self.client
            .log_message(MessageType::INFO, "SurrealQL Analyzer LSP ready")
            .await;
    }

    /// `surrealql-analyzer.toml` changed underneath us — usually because the
    /// "suppress workspace-wide" quick fix was just accepted. Re-resolve the
    /// policy and re-publish every document against it.
    async fn did_change_watched_files(&self, _params: DidChangeWatchedFilesParams) {
        self.publish_all_diagnostics().await;
    }

    /// Acknowledges `shutdown`, and arms the exit the protocol promises.
    ///
    /// The client follows `shutdown` with an `exit` notification and expects
    /// the process to end. tower-lsp answers `exit` by refusing further
    /// requests but keeps reading stdin, so a client that does not also close
    /// the pipe (or whose close is delayed) leaves a live process behind —
    /// one per restart, each holding its analysed workspace. Editors restart
    /// servers freely, so the grace exit below caps that: after `shutdown`
    /// the process ends within two seconds whatever the client does next.
    /// Closing stdin still ends it immediately, as before.
    async fn shutdown(&self) -> Result<()> {
        tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            std::process::exit(0);
        });
        Ok(())
    }

    /// Suppression quick fixes for the diagnostics under the cursor: silence
    /// this one here, or silence the code across the workspace.
    ///
    /// Both write the *canonical* code spelling, not the one the editor
    /// displays — see [`code_action::suppressible_code`] for why those differ
    /// and why copying the displayed one silently does nothing.
    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        // Defense in depth behind the `initialize` gate: a client that never
        // declared literal support gets nothing even if it asks anyway.
        if !self.code_action_literal_support.load(Ordering::Relaxed) {
            return Ok(None);
        }

        let uri = params.text_document.uri.clone();

        // The config decides both whether a directive needs a reason and where
        // the `[lints]` table to edit lives; re-read so an edit made since the
        // handshake (including one of ours) is the one we build on.
        self.reload_config().await;
        let config = self.config.read().await.clone();
        let require_reason = config
            .as_ref()
            .is_some_and(|loaded| loaded.config.diagnostics.require_suppression_reasons);

        let candidates = self.suppressible_diagnostics(&uri, &params).await;
        if candidates.is_empty() {
            return Ok(None);
        }
        let index = {
            let ws = self.workspace.read().await;
            ws.line_index(&uri)
        };

        let mut actions: Vec<CodeActionOrCommand> = Vec::new();
        for (code, diagnostic) in candidates {
            // The title names the code the way the editor spelled it, so the
            // action reads as being about the squiggle the user clicked.
            let shown = match &diagnostic.code {
                Some(NumberOrString::String(rendered)) => rendered.clone(),
                _ => code.clone(),
            };

            if let Some(index) = &index {
                let offset = index.position_to_offset(diagnostic.range.start);
                if self.inline_suppression_possible(&uri, offset).await {
                    actions.push(suppression_action(
                        format!("Suppress {shown} here"),
                        uri.clone(),
                        code_action::inline_suppression_edit(
                            index.text(),
                            offset,
                            &code,
                            require_reason,
                        ),
                        diagnostic.clone(),
                    ));
                }
            }

            // Offered only when a `surrealql-analyzer.toml` already exists: creating
            // one is a resource operation not every client supports, and a
            // quick fix should not invent a workspace's configuration.
            if let Some(loaded) = &config {
                if let Some(edit) = code_action::lints_allow_edit(&loaded.text, &code) {
                    if let Ok(config_uri) = Url::from_file_path(&loaded.path) {
                        actions.push(suppression_action(
                            format!("Suppress {shown} workspace-wide (surrealql-analyzer.toml)"),
                            config_uri,
                            edit,
                            diagnostic.clone(),
                        ));
                    }
                }
            }
        }

        Ok((!actions.is_empty()).then_some(actions))
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri;
        let analysis = {
            let ws = self.workspace.read().await;
            ws.feature_analysis(&uri)
        };
        let Some(analysis) = analysis else {
            return Ok(None);
        };

        // A document that did not parse has no `DEFINE FUNCTION` to ghost a
        // return type onto; its `LET` hints still come from the analysis.
        let return_hints = analysis
            .parsed
            .as_deref()
            .map(|parsed| function_return_hints_parsed(parsed, &analysis.schema))
            .unwrap_or_default();
        let hints = surrealql_analyzer_workspace::let_binding_hints(&analysis.output)
            .into_iter()
            .chain(return_hints)
            .map(|hint| {
                // The grey `: <kind>` sits right after the `$name` token.
                let position = analysis
                    .index
                    .offset_to_position(hint.name_span.range().end() as usize);
                InlayHint {
                    position,
                    label: InlayHintLabel::String(hint.label),
                    kind: Some(InlayHintKind::TYPE),
                    text_edits: None,
                    tooltip: None,
                    padding_left: Some(true),
                    padding_right: Some(false),
                    data: None,
                }
            })
            .collect();

        Ok(Some(hints))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        if let Some(hover) = self.host_hover(&uri, position).await {
            return Ok(Some(hover));
        }
        let analysis = {
            let ws = self.workspace.read().await;
            ws.feature_analysis(&uri)
        };
        let Some(analysis) = analysis else {
            return Ok(None);
        };

        let offset = analysis.index.position_to_offset(position) as u32;
        let Some(info) = hover_from_cache(
            &analysis.output,
            &analysis.schema,
            analysis.parsed.as_deref(),
            &analysis.source,
            &analysis.text,
            offset,
        ) else {
            return Ok(None);
        };

        let range = info.span.range();
        let range = analysis
            .index
            .range(range.start() as usize, range.end() as usize);

        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: info.markdown,
            }),
            range: Some(range),
        }))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri;
        let ws = self.workspace.read().await;

        // Tokens are a function of the document's text alone, and the client
        // re-pulls them for every open document whenever `refresh` is sent —
        // after a save, or a schema edit that moved nothing in this file. The
        // answer is cached against the text it came from, so only a document
        // whose text actually changed is tokenized again (`did_change` hands
        // the document a new text, which is the invalidation).

        // A `.surql` document is tokenized whole, from the parse the analysis
        // cache already holds.
        if let Some((text, parsed)) = ws.parsed_surql(&uri) {
            let index = ws
                .line_index(&uri)
                .unwrap_or_else(|| Arc::new(LineIndex::new(Arc::clone(&text))));
            let data = self.semantic_tokens_for(&uri, &text, || {
                semantic::encode(&index, &semantic::tokens(&parsed))
            });
            return Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
                result_id: None,
                data,
            })));
        }

        // A host document is tokenized only where a query actually is; the
        // surrounding TypeScript belongs to whichever server owns it.
        let Some((text, queries)) = ws.host_queries(&uri) else {
            return Ok(None);
        };
        let index = ws
            .line_index(&uri)
            .unwrap_or_else(|| Arc::new(LineIndex::new(Arc::clone(&text))));
        let data = self.semantic_tokens_for(&uri, &text, || {
            let mut tokens = Vec::new();
            for (index, query) in queries.iter().enumerate() {
                let source = SourceId::new(format!("embedded://{}#{index}", uri.as_str()));
                let Ok(parsed) = parse_source(source, query.text.as_str()) else {
                    continue;
                };
                tokens.extend(semantic::map_to_host(query, semantic::tokens(&parsed)));
            }
            semantic::encode(&index, &tokens)
        });

        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data,
        })))
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let analysis = {
            let ws = self.workspace.read().await;
            ws.completion_analysis(&uri)
        };
        let Some(analysis) = analysis else {
            // The document isn't tracked, so there is nothing to complete
            // against. Logged because it is otherwise indistinguishable, in the
            // editor, from "the server returned no candidates".
            eprintln!(
                "[surrealql-analyzer] completion {}:{} → no analysis for this document",
                position.line + 1,
                position.character
            );
            return Ok(None);
        };

        let offset = analysis.index.position_to_offset(position) as u32;
        let items: Vec<CompletionItem> = surrealql_analyzer_workspace::complete_at(
            &analysis.output,
            &analysis.schema,
            &analysis.parsed,
            offset,
        )
        .into_iter()
        .map(|candidate| completion::candidate_to_item(&analysis.text, candidate))
        .collect();

        // One line per request, so "nothing happened" in the editor can be told
        // apart from "the request never arrived". Shows the text around the
        // cursor, since a stale document is the usual cause of a surprising
        // candidate set.
        let around: String = analysis
            .text
            .get(offset.saturating_sub(12) as usize..(offset as usize + 4).min(analysis.text.len()))
            .unwrap_or("")
            .replace('\n', "⏎");
        eprintln!(
            "[surrealql-analyzer] completion {}:{} (offset {offset}) near {around:?} → {} item(s){}",
            position.line + 1,
            position.character,
            items.len(),
            items
                .first()
                .map(|i| format!(": {}…", i.label))
                .unwrap_or_default()
        );

        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let analysis = {
            let ws = self.workspace.read().await;
            ws.feature_analysis(&uri)
        };
        let Some(analysis) = analysis else {
            return Ok(None);
        };

        let offset = analysis.index.position_to_offset(position) as u32;
        let Some(target) = definition_from_cache(
            &analysis.output,
            &analysis.schema,
            analysis.parsed.as_deref(),
            &analysis.source,
            offset,
        ) else {
            return Ok(None);
        };

        // The definition may live in another `.surql` file; map its source id
        // back to that document's URI and text to build the location.
        let source_key = target.span.source().to_string();
        let Some((def_uri, def_text)) = analysis.sources.get(&source_key) else {
            return Ok(None);
        };

        let range = target.span.range();
        let range = LineIndex::new(Arc::clone(def_text))
            .range(range.start() as usize, range.end() as usize);

        Ok(Some(GotoDefinitionResponse::Scalar(Location {
            uri: def_uri.clone(),
            range,
        })))
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        {
            let mut ws = self.workspace.write().await;
            ws.upsert(params.text_document.uri, params.text_document.text);
        }
        self.publish_diagnostics(&uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        // Snapshot the analysis counters so we can report which path this edit
        // took (full rebuild vs symbol-incremental vs pure cache hit) and how
        // long it cost — printed to stderr, which Zed surfaces in the language
        // server logs. Diagnostic only; safe to remove/gate before release.
        let (full_before, incr_before, reanalyzed_before) = {
            let ws = self.workspace.read().await;
            (
                ws.analyze_run_count(),
                ws.incremental_run_count(),
                ws.reanalyzed_source_count(),
            )
        };
        if let Some(change) = params.content_changes.into_iter().last() {
            let mut ws = self.workspace.write().await;
            ws.upsert(uri.clone(), change.text);
        }
        let started = std::time::Instant::now();
        self.publish_diagnostics(&uri).await;
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let (full_after, incr_after, reanalyzed_after) = {
            let ws = self.workspace.read().await;
            (
                ws.analyze_run_count(),
                ws.incremental_run_count(),
                ws.reanalyzed_source_count(),
            )
        };
        let path = if full_after > full_before {
            "FULL rebuild"
        } else if incr_after > incr_before {
            "incremental"
        } else {
            "cache hit"
        };
        let sources = reanalyzed_after.saturating_sub(reanalyzed_before);
        eprintln!(
            "[surrealql-analyzer] edit → {path} in {elapsed_ms:.1}ms ({sources} source(s) re-analyzed)"
        );
    }

    async fn did_save(&self, _params: DidSaveTextDocumentParams) {
        self.publish_all_diagnostics().await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        {
            let mut ws = self.workspace.write().await;
            ws.remove(&uri);
        }
        if let Ok(mut cache) = self.semantic_cache.lock() {
            cache.remove(&uri);
        }
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }
}
