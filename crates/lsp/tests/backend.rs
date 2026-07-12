//! Protocol-level backend tests: drive the server through its JSON-RPC
//! service (so the tower-lsp state machine runs — publishes are dropped
//! before `initialized`) and read what it emits on the client socket.

use futures::StreamExt;
use serde_json::json;
use tower::{Service, ServiceExt};
use tower_lsp::jsonrpc::{Request, Response};
use tower_lsp::lsp_types::*;
use tower_lsp::LspService;

use surrealguard_lsp::backend::Backend;

struct Server {
    service: LspService<Backend>,
    socket: std::pin::Pin<Box<tower_lsp::ClientSocket>>,
    buffered: Vec<Request>,
}

impl Server {
    async fn started() -> Self {
        let (service, socket) = LspService::new(Backend::new);
        let mut server = Server {
            service,
            socket: Box::pin(socket),
            buffered: Vec::new(),
        };
        server
            .call("initialize", Some(1), json!({"capabilities": {}}))
            .await;
        server.call("initialized", None, json!({})).await;
        server
    }

    /// Sends one request/notification, draining client-bound messages
    /// while it runs (the client channel is bounded — handlers block on
    /// publish until someone reads).
    async fn call(&mut self, method: &'static str, id: Option<i64>, params: serde_json::Value) {
        let mut builder = Request::build(method).params(params);
        if let Some(id) = id {
            builder = builder.id(id);
        }
        let request = builder.finish();
        let service = self.service.ready().await.expect("service ready");
        let call = service.call(request);
        tokio::pin!(call);
        loop {
            tokio::select! {
                outcome = &mut call => {
                    let _: Option<Response> = outcome.expect("call succeeds");
                    return;
                }
                message = self.socket.next() => {
                    if let Some(message) = message {
                        self.buffered.push(message);
                    }
                }
            }
        }
    }

    /// The next buffered or incoming publishDiagnostics notification.
    async fn next_publish(&mut self) -> PublishDiagnosticsParams {
        loop {
            let message = if self.buffered.is_empty() {
                self.socket.next().await.expect("client socket open")
            } else {
                self.buffered.remove(0)
            };
            if message.method() == "textDocument/publishDiagnostics" {
                let (_, _, params) = message.into_parts();
                return serde_json::from_value(params.expect("params present"))
                    .expect("publishDiagnostics params decode");
            }
        }
    }
}

fn did_open(uri: &Url, text: &str) -> serde_json::Value {
    json!({
        "textDocument": {
            "uri": uri, "languageId": "surrealql", "version": 1, "text": text,
        }
    })
}

#[tokio::test]
async fn did_open_publishes_diagnostics_and_did_close_clears_them() {
    let mut server = Server::started().await;
    let uri = Url::parse("file:///workspace/query.surql").expect("valid url");

    server
        .call(
            "textDocument/didOpen",
            None,
            did_open(&uri, "SELECT * FROM persn;\n"),
        )
        .await;
    let published = server.next_publish().await;
    assert_eq!(published.uri, uri);
    assert_eq!(published.diagnostics.len(), 1);
    let diagnostic = &published.diagnostics[0];
    assert_eq!(
        diagnostic.code,
        Some(NumberOrString::String("E1001".into()))
    );
    assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::ERROR));
    assert_eq!(diagnostic.range.start.line, 0);
    assert_eq!(diagnostic.range.start.character, 14);

    server
        .call(
            "textDocument/didClose",
            None,
            json!({"textDocument": {"uri": uri}}),
        )
        .await;
    let cleared = server.next_publish().await;
    assert_eq!(cleared.uri, uri);
    assert!(cleared.diagnostics.is_empty());
}

#[tokio::test]
async fn did_change_reanalyzes_with_the_new_text() {
    let mut server = Server::started().await;
    let uri = Url::parse("file:///workspace/query.surql").expect("valid url");

    server
        .call(
            "textDocument/didOpen",
            None,
            did_open(&uri, "SELECT * FROM persn;\n"),
        )
        .await;
    assert_eq!(server.next_publish().await.diagnostics.len(), 1);

    server
        .call(
            "textDocument/didChange",
            None,
            json!({
                "textDocument": {"uri": uri, "version": 2},
                "contentChanges": [
                    {"text": "DEFINE TABLE persn;\nSELECT * FROM persn;\n"}
                ],
            }),
        )
        .await;
    let published = server.next_publish().await;
    assert!(
        !published
            .diagnostics
            .iter()
            .any(|d| d.severity == Some(DiagnosticSeverity::ERROR)),
        "defining the table fixes the error: {:?}",
        published.diagnostics
    );
}

#[tokio::test]
async fn schema_in_one_document_resolves_queries_in_another() {
    let mut server = Server::started().await;
    let schema_uri = Url::parse("file:///workspace/a_schema.surql").expect("valid url");
    let query_uri = Url::parse("file:///workspace/b_query.surql").expect("valid url");

    server
        .call(
            "textDocument/didOpen",
            None,
            did_open(&schema_uri, "DEFINE TABLE person;\n"),
        )
        .await;
    let _ = server.next_publish().await;

    server
        .call(
            "textDocument/didOpen",
            None,
            did_open(&query_uri, "SELECT * FROM person;\n"),
        )
        .await;
    let published = server.next_publish().await;
    assert_eq!(published.uri, query_uri);
    assert!(
        !published
            .diagnostics
            .iter()
            .any(|d| d.severity == Some(DiagnosticSeverity::ERROR)),
        "cross-document schema resolves: {:?}",
        published.diagnostics
    );
}
