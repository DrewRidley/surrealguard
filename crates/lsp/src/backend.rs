//! LSP backend — implements the `LanguageServer` trait.

use std::path::Path;

use surrealguard_diagnostics::PolicyConfig;
use surrealguard_workspace::config::WorkspaceConfig;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::diagnostics;
use crate::workspace::Workspace;

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
        if let Some(change) = params.content_changes.into_iter().last() {
            let mut ws = self.workspace.write().await;
            ws.upsert(uri.clone(), change.text);
        }
        self.publish_diagnostics(&uri).await;
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
