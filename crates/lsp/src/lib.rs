//! SurrealGuard's LSP: workspace document tracking, analysis through
//! `surrealguard-workspace`, and finding-to-diagnostic conversion. The
//! `surrealguard-lsp` binary serves [`backend::Backend`] over stdio.

pub mod backend;
pub mod completion;
pub mod diagnostics;
pub mod semantic;
pub mod text;
pub mod workspace;
