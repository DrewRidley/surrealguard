//! LSP backend — implements the `LanguageServer` trait.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use surrealguard_diagnostics::PolicyConfig;
use surrealguard_syntax::parse::{parse_source, ParsedSource};
use surrealguard_syntax::source::SourceId;
use surrealguard_workspace::query::{
    definition_at_lowered, definition_at_parsed, function_return_hints_parsed, hover_at_lowered,
    hover_at_parsed, DefinitionTarget, HoverInfo,
};
use surrealguard_workspace::{AnalysisOutput, SchemaIndex};
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::workspace::{load_workspace_config, Workspace};
use crate::{completion, diagnostics, semantic};

/// The language server: holds the LSP client handle, the tracked workspace,
/// and the severity policy resolved from `surrealguard.toml`. Implements
/// [`tower_lsp::LanguageServer`].
pub struct Backend {
    client: Client,
    workspace: RwLock<Workspace>,
    /// Severity policy from `surrealguard.toml`, so `[lints]` levels apply in
    /// the editor exactly as they do in `surrealguard check`. Defaults until
    /// `initialize` locates a config in a workspace root.
    policy: RwLock<PolicyConfig>,
    /// State of the one `workspace/semanticTokens/refresh` request that may
    /// be outstanding at a time; see [`Self::refresh_semantic_tokens`].
    /// Shared with the task that sends it.
    refresh: Arc<RefreshState>,
    /// The last semantic-token answer per document, keyed by the text it was
    /// computed from; see [`Self::semantic_tokens_full`].
    semantic_cache: Mutex<HashMap<Url, SemanticEntry>>,
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
        }
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

        // Presentation policy (from surrealguard.toml) applies here, at the
        // consumption edge; the findings themselves carry only their
        // intrinsic class.
        let policy = self.policy.read().await;
        let lsp_diagnostics: Vec<Diagnostic> = result
            .diagnostics
            .iter()
            .filter_map(|d| {
                diagnostics::workspace_finding_to_lsp_diagnostic(
                    &result.source,
                    d,
                    &policy,
                    &result.texts,
                )
            })
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

    /// Hover for a position inside a host file's embedded query: the cursor is
    /// translated into the query, answered by the query's own analysis, and the
    /// resulting span mapped back onto the host text — so a field inside a
    /// `` surql`…` `` template reports the same type it would in a `.surql`
    /// file. `None` for `.surql` documents and for a host position outside
    /// every embedded query, both of which the ordinary path handles.
    async fn host_hover(&self, uri: &Url, position: Position) -> Option<Hover> {
        let (host_text, analysis) = {
            let ws = self.workspace.read().await;
            let host_text = ws.document_text(uri)?;
            let offset = crate::text::position_to_offset(&host_text, position);
            (host_text, ws.host_feature_analysis(uri, offset)?)
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
            range: Some(crate::text::byte_range_to_lsp(
                &host_text, host.start, host.end,
            )),
        })
    }

    /// Publish diagnostics for every tracked document. The first `.surql`
    /// document warms the shared whole-workspace analysis once; every later
    /// one reads the cache. Documents are analyzed and published one at a
    /// time — never a list of every result at once — so the sweep holds one
    /// document's findings in flight, not the workspace's. On a 500-file
    /// workspace the old all-at-once list, each entry carrying its own copy
    /// of every text, was the difference between 90 MB and 900 MB resident.
    async fn publish_all_diagnostics(&self) {
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

        if let Some(folders) = &params.workspace_folders {
            let roots: Vec<_> = folders
                .iter()
                .filter_map(|f| f.uri.to_file_path().ok())
                .collect();

            // The first workspace root that carries a surrealguard.toml
            // speaks for the workspace: its `[lints]` levels become the
            // editor's policy and its `[sources]` globs decide what the scan
            // loads, so the editor honors the same config as
            // `surrealguard check`. No config leaves the defaults.
            let config = roots.iter().find_map(|root| load_workspace_config(root));
            if let Some(config) = &config {
                *self.policy.write().await = config.policy();
            }

            let mut ws = self.workspace.write().await;
            ws.roots = roots;
            if let Some(config) = config {
                // `[analysis]` (the target SurrealDB version behind the 8xxx
                // checks) and `[diagnostics]` are analysis inputs, not
                // presentation: they go into the analysis itself.
                ws.scan_folders(Some(&config.sources));
                ws.set_config(config);
            } else {
                ws.scan_folders(None);
            }
        }

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "surrealguard-lsp".to_string(),
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
                ..ServerCapabilities::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.publish_all_diagnostics().await;
        self.client
            .log_message(MessageType::INFO, "SurrealGuard LSP ready")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
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
        let hints = surrealguard_workspace::let_binding_hints(&analysis.output)
            .into_iter()
            .chain(return_hints)
            .map(|hint| {
                // The grey `: <kind>` sits right after the `$name` token.
                let position = crate::text::offset_to_position(
                    &analysis.text,
                    hint.name_span.range().end() as usize,
                );
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

        let offset = crate::text::position_to_offset(&analysis.text, position) as u32;
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
        let range = crate::text::byte_range_to_lsp(
            &analysis.text,
            range.start() as usize,
            range.end() as usize,
        );

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
            let data = self.semantic_tokens_for(&uri, &text, || {
                semantic::encode(&text, &semantic::tokens(&parsed))
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
        let data = self.semantic_tokens_for(&uri, &text, || {
            let mut tokens = Vec::new();
            for (index, query) in queries.iter().enumerate() {
                let source = SourceId::new(format!("embedded://{}#{index}", uri.as_str()));
                let Ok(parsed) = parse_source(source, query.text.as_str()) else {
                    continue;
                };
                tokens.extend(semantic::map_to_host(query, semantic::tokens(&parsed)));
            }
            semantic::encode(&text, &tokens)
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
                "[surrealguard] completion {}:{} → no analysis for this document",
                position.line + 1,
                position.character
            );
            return Ok(None);
        };

        let offset = crate::text::position_to_offset(&analysis.text, position) as u32;
        let items: Vec<CompletionItem> = surrealguard_workspace::complete_at(
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
            "[surrealguard] completion {}:{} (offset {offset}) near {around:?} → {} item(s){}",
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

        let offset = crate::text::position_to_offset(&analysis.text, position) as u32;
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
        let range =
            crate::text::byte_range_to_lsp(def_text, range.start() as usize, range.end() as usize);

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
            "[surrealguard] edit → {path} in {elapsed_ms:.1}ms ({sources} source(s) re-analyzed)"
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
