//! LSP backend — implements the `LanguageServer` trait.

use std::path::Path;

use surrealguard_diagnostics::PolicyConfig;
use surrealguard_workspace::config::WorkspaceConfig;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::workspace::Workspace;
use crate::{completion, diagnostics};

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
}

impl Backend {
    /// Builds a backend bound to the given LSP client, with an empty
    /// workspace and the default (no-config) policy.
    pub fn new(client: Client) -> Self {
        Self {
            client,
            workspace: RwLock::new(Workspace::new()),
            policy: RwLock::new(PolicyConfig::default()),
        }
    }

    /// Analyze a document and publish diagnostics.
    async fn publish_diagnostics(&self, uri: &Url) {
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

        self.client
            .publish_diagnostics(uri.clone(), lsp_diagnostics, None)
            .await;
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

        let info = surrealguard_workspace::hover_at(
            &analysis.output,
            &analysis.schema,
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

    /// Publish diagnostics for all tracked documents in a single workspace
    /// analysis pass (avoids re-analyzing the whole workspace once per file).
    async fn publish_all_diagnostics(&self) {
        let results = {
            let ws = self.workspace.read().await;
            ws.analyze_all()
        };
        let policy = self.policy.read().await;
        for (uri, result) in results {
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
            self.client
                .publish_diagnostics(uri, lsp_diagnostics, None)
                .await;
        }
    }
}

/// Loads and parses `surrealguard.toml` from a workspace root. Returns
/// `None` when the root has no config file; a malformed config is treated as
/// absent (the editor falls back to the default policy rather than failing to
/// start).
fn load_workspace_config(root: &Path) -> Option<WorkspaceConfig> {
    let config_path = root.join("surrealguard.toml");
    let text = std::fs::read_to_string(config_path).ok()?;
    WorkspaceConfig::from_toml_str(&text).ok()
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        if let Some(folders) = &params.workspace_folders {
            let roots: Vec<_> = folders
                .iter()
                .filter_map(|f| f.uri.to_file_path().ok())
                .collect();

            // Resolve `[lints]` levels from the first workspace root that
            // carries a surrealguard.toml, so the editor honors the same
            // policy as `surrealguard check`. No config leaves the default.
            if let Some(config) = roots.iter().find_map(|root| load_workspace_config(root)) {
                *self.policy.write().await = config.policy();
            }

            let mut ws = self.workspace.write().await;
            ws.roots = roots;
            ws.scan_folders();
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

        let hints = surrealguard_workspace::let_binding_hints(&analysis.output)
            .into_iter()
            .chain(surrealguard_workspace::function_return_hints(
                &analysis.text,
                &analysis.source,
                &analysis.schema,
            ))
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
        let Some(info) = surrealguard_workspace::hover_at(
            &analysis.output,
            &analysis.schema,
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
        let Some(target) = surrealguard_workspace::definition_at(
            &analysis.output,
            &analysis.schema,
            &analysis.source,
            &analysis.text,
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
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }
}
