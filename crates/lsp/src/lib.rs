//! SurrealQL Analyzer's LSP: workspace document tracking, analysis through
//! `surrealql-analyzer-workspace`, and finding-to-diagnostic conversion. The
//! `surrealql-analyzer-lsp` binary serves [`backend::Backend`] over stdio.

pub mod backend;
pub mod code_action;
pub mod completion;
pub mod diagnostics;
pub mod semantic;
pub mod text;
pub mod workspace;
