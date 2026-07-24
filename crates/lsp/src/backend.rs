//! LSP backend — implements the `LanguageServer` trait.

use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::diagnostics;
use crate::workspace::Workspace;

/// The language server: holds the LSP client handle and the tracked
/// workspace, and implements [`tower_lsp::LanguageServer`].
pub struct Backend {
    client: Client,
    workspace: RwLock<Workspace>,
}

impl Backend {
    /// Builds a backend bound to the given LSP client, with an empty
    /// workspace.
    pub fn new(client: Client) -> Self {
        Self {
            client,
            workspace: RwLock::new(Workspace::new()),
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

        // Presentation policy applies here, at the consumption edge; the
        // findings themselves carry only their intrinsic class.
        let policy = surrealguard_diagnostics::PolicyConfig::default();
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
        let policy = surrealguard_diagnostics::PolicyConfig::default();
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

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        if let Some(folders) = &params.workspace_folders {
            let mut ws = self.workspace.write().await;
            ws.roots = folders
                .iter()
                .filter_map(|f| f.uri.to_file_path().ok())
                .collect();
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
