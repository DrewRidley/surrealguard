use tower_lsp::{LspService, Server};

mod backend;
mod completions;
mod definition;
mod diagnostics;
pub mod hover;
mod signature;
mod symbols;
mod text;
mod workspace;

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(backend::Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
