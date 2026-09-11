//! Shared helpers for analyzer test modules.

/// Runs `f` with a fresh empty-schema context — the boilerplate shared by
/// leaf-analyzer tests.
pub fn with_ctx<T>(f: impl FnOnce(&mut crate::analyzer::context::AnalysisContext<'_>) -> T) -> T {
    let schema = crate::schema::SchemaIndex::default();
    let mut diagnostics: Vec<surrealql_analyzer_diagnostics::Finding> = Vec::new();
    let mut ctx = crate::analyzer::context::AnalysisContext::new(
        &schema,
        surrealql_analyzer_syntax::source::SourceId::new("test"),
        "",
        &mut diagnostics,
    );
    f(&mut ctx)
}

/// A synthetic call for analyzers that only read the argument kinds.
pub fn synthetic_call(path: &str) -> surrealql_analyzer_syntax::ast::Call {
    surrealql_analyzer_syntax::ast::Call {
        path: surrealql_analyzer_syntax::ast::Spanned::new(
            path.to_string(),
            surrealql_analyzer_syntax::span::ByteRange::new(0, 0).expect("empty range is ordered"),
        ),
        written: path.to_string(),
        args: Vec::new(),
    }
}
