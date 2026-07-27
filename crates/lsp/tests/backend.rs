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

    /// Sends one request and returns its decoded result value, draining
    /// client-bound messages while it runs.
    async fn request(
        &mut self,
        method: &'static str,
        id: i64,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let request = Request::build(method).id(id).params(params).finish();
        let service = self.service.ready().await.expect("service ready");
        let call = service.call(request);
        tokio::pin!(call);
        loop {
            tokio::select! {
                outcome = &mut call => {
                    let response: Response = outcome.expect("call succeeds").expect("has response");
                    let (_, result) = response.into_parts();
                    return result.expect("result present");
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

#[tokio::test]
async fn goto_definition_jumps_from_a_reference_into_the_schema_file() {
    let mut server = Server::started().await;
    let schema_uri = Url::parse("file:///workspace/a_schema.surql").expect("valid url");
    let query_uri = Url::parse("file:///workspace/b_query.surql").expect("valid url");

    server
        .call(
            "textDocument/didOpen",
            None,
            did_open(
                &schema_uri,
                "DEFINE TABLE user;\nDEFINE FIELD name ON user TYPE string;\n",
            ),
        )
        .await;
    let _ = server.next_publish().await;

    let query = "SELECT name FROM user;\n";
    server
        .call("textDocument/didOpen", None, did_open(&query_uri, query))
        .await;
    let _ = server.next_publish().await;

    // Cmd-click on `user` in `FROM user` → the DEFINE TABLE in the schema file.
    let table_char = query.find("user").expect("table present") as u32;
    let table_location: Location = serde_json::from_value(
        server
            .request(
                "textDocument/definition",
                2,
                json!({
                    "textDocument": {"uri": query_uri},
                    "position": {"line": 0, "character": table_char},
                }),
            )
            .await,
    )
    .expect("definition returns a Location");
    assert_eq!(table_location.uri, schema_uri);
    // `user` is the DEFINE TABLE name at columns 12..16 of line 0.
    assert_eq!(table_location.range.start.line, 0);
    assert_eq!(table_location.range.start.character, 13);

    // Cmd-click on the projected `name` → the DEFINE FIELD in the schema file.
    let field_char = query.find("name").expect("field present") as u32;
    let field_location: Location = serde_json::from_value(
        server
            .request(
                "textDocument/definition",
                3,
                json!({
                    "textDocument": {"uri": query_uri},
                    "position": {"line": 0, "character": field_char},
                }),
            )
            .await,
    )
    .expect("definition returns a Location");
    assert_eq!(field_location.uri, schema_uri);
    // `name` is the DEFINE FIELD name on line 1, at columns 12..16.
    assert_eq!(field_location.range.start.line, 1);
    assert_eq!(field_location.range.start.character, 13);
}

#[tokio::test]
async fn embedded_surql_in_svelte_publishes_findings_at_host_spans() {
    let mut server = Server::started().await;
    let schema_uri = Url::parse("file:///workspace/schema.surql").expect("valid url");
    let svelte_uri = Url::parse("file:///workspace/App.svelte").expect("valid url");

    server
        .call(
            "textDocument/didOpen",
            None,
            did_open(
                &schema_uri,
                "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\n",
            ),
        )
        .await;
    let _ = server.next_publish().await;

    let svelte = "<h1>People</h1>\n<script lang=\"ts\">\nconst rows = db.query(`SELECT * FROM persn WHERE name = ${filter}`);\n</script>\n";
    server
        .call(
            "textDocument/didOpen",
            None,
            json!({
                "textDocument": {
                    "uri": svelte_uri, "languageId": "svelte", "version": 1, "text": svelte,
                }
            }),
        )
        .await;
    let published = server.next_publish().await;
    assert_eq!(published.uri, svelte_uri);

    let error = published
        .diagnostics
        .iter()
        .find(|d| d.code == Some(NumberOrString::String("E1001".into())))
        .expect("unknown-table finding reaches the host file");
    // The squiggle sits on `persn` inside the template, line 2 of the
    // svelte file.
    assert_eq!(error.range.start.line, 2);
    let line = svelte.lines().nth(2).expect("line exists");
    let start = error.range.start.character as usize;
    let end = error.range.end.character as usize;
    assert_eq!(&line[start..end], "persn");
    assert!(error.message.contains("did you mean `person`?") || error.message.contains("help:"));
}

/// Opens a schema document plus a query document and returns the server, so
/// the completion tests share one setup.
async fn server_with_schema(query_uri: &Url, query: &str) -> Server {
    let mut server = Server::started().await;
    let schema_uri = Url::parse("file:///workspace/a_schema.surql").expect("valid url");
    server
        .call(
            "textDocument/didOpen",
            None,
            did_open(
                &schema_uri,
                "DEFINE TABLE person SCHEMAFULL;\n\
                 DEFINE FIELD name ON person TYPE string;\n\
                 DEFINE FIELD status ON person TYPE string;\n\
                 DEFINE FIELD age ON person TYPE int;\n\
                 DEFINE TABLE company SCHEMAFULL;\n",
            ),
        )
        .await;
    let _ = server.next_publish().await;
    server
        .call("textDocument/didOpen", None, did_open(query_uri, query))
        .await;
    let _ = server.next_publish().await;
    server
}

#[tokio::test]
async fn the_server_advertises_completion_with_surrealql_trigger_characters() {
    let (service, socket) = LspService::new(Backend::new);
    let mut server = Server {
        service,
        socket: Box::pin(socket),
        buffered: Vec::new(),
    };
    let result: InitializeResult = serde_json::from_value(
        server
            .request("initialize", 1, json!({"capabilities": {}}))
            .await,
    )
    .expect("initialize result decodes");

    let completion = result
        .capabilities
        .completion_provider
        .expect("the server must advertise completion");
    let triggers = completion.trigger_characters.expect("trigger characters");
    // `$` opens a param, `.` a member, `:` completes a `::` function path.
    for expected in ["$", ".", ":"] {
        assert!(
            triggers.contains(&expected.to_string()),
            "missing {expected}"
        );
    }
    assert_eq!(completion.resolve_provider, Some(false));
}

#[tokio::test]
async fn completion_returns_ranked_items_with_types_and_explicit_edit_ranges() {
    let query_uri = Url::parse("file:///workspace/b_query.surql").expect("valid url");
    let query = "SELECT sta FROM person;\n";
    let mut server = server_with_schema(&query_uri, query).await;

    let items: Vec<CompletionItem> = serde_json::from_value(
        server
            .request(
                "textDocument/completion",
                7,
                json!({
                    "textDocument": {"uri": query_uri},
                    // Right after the typed `sta`.
                    "position": {"line": 0, "character": 10},
                }),
            )
            .await,
    )
    .expect("completion returns an item array");

    let first = items.first().expect("at least one item");
    assert_eq!(first.label, "status");
    assert_eq!(first.kind, Some(CompletionItemKind::FIELD));
    assert_eq!(first.detail.as_deref(), Some("string"));
    assert_eq!(first.sort_text.as_deref(), Some("0000"));

    // The edit replaces the whole `sta` token, so accepting leaves no tail.
    let Some(CompletionTextEdit::Edit(edit)) = &first.text_edit else {
        panic!("items must carry an explicit edit");
    };
    assert_eq!(edit.range.start, Position::new(0, 7));
    assert_eq!(edit.range.end, Position::new(0, 10));
    assert_eq!(edit.new_text, "status");

    // The client must be able to keep the server's order.
    let sort_texts: Vec<&str> = items
        .iter()
        .map(|item| item.sort_text.as_deref().expect("sort_text set"))
        .collect();
    let mut sorted = sort_texts.clone();
    sorted.sort_unstable();
    assert_eq!(sort_texts, sorted);
}

#[tokio::test]
async fn completion_on_an_unparseable_statement_still_offers_the_tables() {
    let query_uri = Url::parse("file:///workspace/b_query.surql").expect("valid url");
    // Mid-keystroke: nothing after FROM.
    let query = "SELECT name FROM \n";
    let mut server = server_with_schema(&query_uri, query).await;

    let items: Vec<CompletionItem> = serde_json::from_value(
        server
            .request(
                "textDocument/completion",
                8,
                json!({
                    "textDocument": {"uri": query_uri},
                    "position": {"line": 0, "character": 17},
                }),
            )
            .await,
    )
    .expect("completion returns an item array");

    let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
    assert!(labels.contains(&"person"), "{labels:?}");
    assert!(labels.contains(&"company"), "{labels:?}");
    assert!(items
        .iter()
        .filter(|item| item.label == "person")
        .all(|item| item.kind == Some(CompletionItemKind::CLASS)));
}
