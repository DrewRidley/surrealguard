//! Shared helpers for analyzer test modules.

/// Runs `f` with a fresh empty-schema context — the boilerplate shared by
/// leaf-analyzer tests.
pub fn with_ctx<T>(f: impl FnOnce(&mut crate::analyzer::context::AnalysisContext<'_>) -> T) -> T {
    let schema = crate::schema::SchemaIndex::default();
    let mut diagnostics: Vec<surrealguard_diagnostics::Finding> = Vec::new();
    let mut ctx = crate::analyzer::context::AnalysisContext::new(
        &schema,
        surrealguard_syntax::source::SourceId::new("test"),
        "",
        &mut diagnostics,
    );
    f(&mut ctx)
}

/// A synthetic call for analyzers that only read the argument kinds.
pub fn synthetic_call(path: &str) -> surrealguard_syntax::ast::Call {
    surrealguard_syntax::ast::Call {
        path: surrealguard_syntax::ast::Spanned::new(
            path.to_string(),
            surrealguard_syntax::span::ByteRange::new(0, 0).expect("empty range is ordered"),
        ),
        args: Vec::new(),
    }
}
