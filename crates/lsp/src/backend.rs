//! LSP backend — implements the LanguageServer trait.

use std::path::PathBuf;

use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use surrealguard_analyzer::hints::{collect_type_hints, TypeHintKind};

use crate::definition;
use crate::diagnostics;
use crate::hover;
use crate::text::{byte_range_to_lsp, offset_to_position};
use crate::workspace::Workspace;

pub struct Backend {
    client: Client,
    workspace: RwLock<Workspace>,
}

impl Backend {
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
            ws.analyze_document(uri)
        };

        let Some(result) = result else {
            return;
        };

        let lsp_diagnostics: Vec<Diagnostic> = result
            .diagnostics
            .iter()
            .map(|d| diagnostics::to_lsp_diagnostic(&result.source, &result.uri, d))
            .collect();

        self.client
            .publish_diagnostics(uri.clone(), lsp_diagnostics, None)
            .await;
    }

    /// Publish diagnostics for all open documents.
    async fn publish_all_diagnostics(&self) {
        let uris: Vec<Url> = {
            let ws = self.workspace.read().await;
            ws.documents().map(|d| d.uri.clone()).collect()
        };
        for uri in uris {
            self.publish_diagnostics(&uri).await;
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Extract workspace folders
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
                definition_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                inlay_hint_provider: Some(OneOf::Left(true)),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec![
                        ".".into(), ":".into(), "$".into(),
                        ">".into(), " ".into(),
                    ]),
                    ..CompletionOptions::default()
                }),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".into(), ",".into()]),
                    retrigger_characters: None,
                    work_done_progress_options: WorkDoneProgressOptions {
                        work_done_progress: None,
                    },
                }),
                document_symbol_provider: Some(OneOf::Left(true)),
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
            ws.upsert(
                params.text_document.uri,
                params.text_document.text,
                params.text_document.version,
            );
        }
        self.publish_diagnostics(&uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        if let Some(change) = params.content_changes.into_iter().last() {
            let mut ws = self.workspace.write().await;
            ws.upsert(uri.clone(), change.text, params.text_document.version);
        }
        self.publish_diagnostics(&uri).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        // Re-analyze all documents since schema might have changed
        self.publish_all_diagnostics().await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        // Clear diagnostics for the closed file
        self.client
            .publish_diagnostics(params.text_document.uri, Vec::new(), None)
            .await;
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        let result = {
            let ws = self.workspace.read().await;
            ws.analyze_document(&uri)
        };

        let Some(result) = result else {
            return Ok(None);
        };

        let items = crate::completions::resolve(&result.source, position, &result.context);
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let result = {
            let ws = self.workspace.read().await;
            ws.analyze_document(&uri)
        };

        let Some(result) = result else {
            return Ok(None);
        };

        Ok(hover::resolve(&result.source, position, &result.context))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let result = {
            let ws = self.workspace.read().await;
            ws.analyze_document(&uri)
        };

        let Some(result) = result else {
            return Ok(None);
        };

        Ok(definition::resolve(
            &result.source,
            position,
            &result.context,
            &uri,
        ))
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let result = {
            let ws = self.workspace.read().await;
            ws.analyze_document(&uri)
        };

        let Some(result) = result else {
            return Ok(None);
        };

        Ok(crate::signature::resolve(
            &result.source,
            position,
            &result.context,
        ))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;

        let source = {
            let ws = self.workspace.read().await;
            ws.get(&uri).map(|d| d.text.clone())
        };

        let Some(source) = source else {
            return Ok(None);
        };

        let symbols = crate::symbols::document_symbols(&source).unwrap_or_default();
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri;

        let result = {
            let ws = self.workspace.read().await;
            ws.analyze_document(&uri)
        };

        let Some(mut result) = result else {
            return Ok(None);
        };

        let tree = match surrealguard_analyzer::parse(&result.source) {
            Ok(tree) => tree,
            Err(_) => return Ok(None),
        };
        let root = tree.root_node();

        let hints = collect_type_hints(&root, &result.source, &mut result.context);
        let lsp_hints: Vec<InlayHint> = hints
            .into_iter()
            .map(|hint| {
                let position = offset_to_position(&result.source, hint.position as usize);
                InlayHint {
                    position,
                    label: InlayHintLabel::String(hint.label),
                    kind: Some(InlayHintKind::TYPE),
                    text_edits: None,
                    tooltip: None,
                    padding_left: Some(matches!(hint.kind, TypeHintKind::Statement)),
                    padding_right: None,
                    data: None,
                }
            })
            .collect();

        Ok(Some(lsp_hints))
    }
}
