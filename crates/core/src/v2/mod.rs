//! v2 analyzer — tree-sitter based, with spans on every diagnostic.
//!
//! This replaces the v1 analyzer's dependency on `surrealdb::sql::parse()`
//! with tree-sitter parsing, giving us:
//! - Precise byte spans on every node (for editor diagnostics)
//! - Error recovery (partial results from malformed queries)
//! - No dependency on surrealdb's internal, unstable AST types
//! - Our own stable type system

pub mod context;
pub mod diagnostic;
pub mod parser;
pub mod schema;
pub mod span;
pub mod types;

use context::Context;
use diagnostic::Diagnostic;
use types::Type;

/// Result of analyzing a SurrealQL source string.
#[derive(Debug)]
pub struct AnalysisResult {
    /// The inferred return type of the query.
    pub typ: Type,
    /// All diagnostics (errors, warnings, hints) with spans.
    pub diagnostics: Vec<Diagnostic>,
    /// Whether the analysis completed without errors.
    pub has_errors: bool,
}

/// Analyze a SurrealQL source string against a schema context.
///
/// This is the v2 entry point, replacing v1's `analyze()`.
///
/// # Arguments
/// * `ctx` — Schema context (populated by prior DEFINE statements)
/// * `source` — The SurrealQL source text to analyze
///
/// # Returns
/// An [`AnalysisResult`] containing the inferred type and diagnostics.
pub fn analyze(ctx: &mut Context, source: &str) -> AnalysisResult {
    let tree = match parser::parse(source) {
        Ok(tree) => tree,
        Err(e) => {
            return AnalysisResult {
                typ: Type::Any,
                diagnostics: vec![Diagnostic::error(
                    span::Span::new(0, source.len() as u32),
                    diagnostic::DiagnosticCode::ParseError,
                    e.to_string(),
                )],
                has_errors: true,
            };
        }
    };

    let root = tree.root_node();
    let mut diagnostics = Vec::new();

    // Check for parse errors in the tree
    if root.has_error() {
        collect_parse_errors(&root, source, &mut diagnostics);
    }

    // Extract schema definitions (DEFINE TABLE/FIELD) into context
    schema::extract_schema(&root, source, ctx);

    // TODO: Analyze statements (SELECT, CREATE, UPDATE, etc.)
    // This is where the statement-level analysis will go.

    let has_errors = diagnostics.iter().any(|d| d.severity == diagnostic::Severity::Error);
    AnalysisResult {
        typ: Type::Null,
        diagnostics,
        has_errors,
    }
}

/// Collect parse error nodes from the CST.
fn collect_parse_errors(node: &tree_sitter::Node, source: &str, diagnostics: &mut Vec<Diagnostic>) {
    if node.is_error() || node.is_missing() {
        let span = span::Span::from_node(node);
        let text = if span.len() > 0 {
            span.text(source)
        } else {
            "<missing>"
        };
        diagnostics.push(Diagnostic::error(
            span,
            diagnostic::DiagnosticCode::ParseError,
            format!("Syntax error near '{}'", text),
        ));
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.has_error() || child.is_error() || child.is_missing() {
            collect_parse_errors(&child, source, diagnostics);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyze_schema_only() {
        let mut ctx = Context::new();
        let result = analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            "#,
        );
        assert!(!result.has_errors);
        assert!(ctx.has_table("user"));
        assert_eq!(ctx.get_field("user", "name").unwrap().typ, Some(types::Type::String));
        assert_eq!(ctx.get_field("user", "age").unwrap().typ, Some(types::Type::Int));
    }

    #[test]
    fn analyze_with_parse_error() {
        let mut ctx = Context::new();
        // Use a query that tree-sitter can't recover from
        let result = analyze(&mut ctx, "SELECT FROM WHERE @@ ###");
        assert!(result.has_errors);
        assert!(!result.diagnostics.is_empty());
        assert_eq!(
            result.diagnostics[0].code,
            diagnostic::DiagnosticCode::ParseError
        );
    }

    #[test]
    fn analyze_multiple_define_statements() {
        let mut ctx = Context::new();
        let result = analyze(
            &mut ctx,
            r#"
            DEFINE TABLE organization SCHEMAFULL;
            DEFINE FIELD name ON organization TYPE string;

            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD email ON user TYPE string;
            DEFINE FIELD org ON user TYPE record<organization>;
            "#,
        );
        assert!(!result.has_errors);
        assert!(ctx.has_table("organization"));
        assert!(ctx.has_table("user"));
        assert_eq!(
            ctx.get_field("user", "org").unwrap().typ,
            Some(types::Type::Record(vec!["organization".to_string()]))
        );
    }

    #[test]
    fn diagnostic_has_span() {
        let mut ctx = Context::new();
        let source = "SELECT * FROM ;";
        let result = analyze(&mut ctx, source);
        for diag in &result.diagnostics {
            // Every diagnostic should have a valid span
            assert!(diag.span.end <= source.len() as u32);
        }
    }
}
