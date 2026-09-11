//! The `surrealql-analyzer-lsp` binary: serves the analyzer over stdio.

use tower_lsp::{LspService, Server};

use surrealql_analyzer_lsp::backend::Backend;

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
