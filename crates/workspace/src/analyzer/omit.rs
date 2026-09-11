//! `OMIT` without a wildcard projection (4012).
//!
//! `OMIT` strips fields from the rows a `SELECT` returns *after* the
//! projection is computed. Under `*` that is the only way to leave a column
//! out; under an explicit projection list the author already chose every
//! column, so an `OMIT` either names a field that is not in the output (a
//! no-op) or names one that is (projected, then thrown away). Neither is
//! what a reader takes the clause to mean, and both have the same fix: say
//! what to return, not what to hide.
//!
//! The grammar accepts `SELECT a, b OMIT c FROM t` (verified against the
//! vendored tree-sitter grammar: `OmitClause` lowers alongside any `Fields`),
//! so this is an analyzer contract, not a parse error.

use surrealql_analyzer_syntax::ast;
use surrealql_analyzer_syntax::ast::visit::{self, Visitor};
use surrealql_analyzer_syntax::span::SourceSpan;

use crate::analyzer::context::AnalysisContext;

/// Walks one top-level statement (and every SELECT nested in it) and reports
/// each `OMIT` idiom whose statement projects no `*`.
pub(crate) fn check_statement(
    ctx: &mut AnalysisContext<'_>,
    statement: &ast::Spanned<ast::Statement>,
) {
    let mut walker = OmitWithoutWildcard { ctx };
    walker.visit_statement(statement);
}

struct OmitWithoutWildcard<'c, 'a> {
    ctx: &'c mut AnalysisContext<'a>,
}

impl OmitWithoutWildcard<'_, '_> {
    fn text(&self, span: surrealql_analyzer_syntax::span::ByteRange) -> &str {
        self.ctx
            .source_text()
            .get(span.start() as usize..span.end() as usize)
            .unwrap_or("")
            .trim()
    }
}

impl Visitor for OmitWithoutWildcard<'_, '_> {
    fn visit_select(&mut self, select: &ast::SelectStmt) {
        let has_wildcard = select
            .projections
            .iter()
            .any(|projection| matches!(projection, ast::Projection::Wildcard(_)));
        if !has_wildcard && !select.omit.is_empty() {
            // The output keys an explicit projection produces: the alias when
            // there is one, else the projected expression as written.
            let projected: Vec<String> = select
                .projections
                .iter()
                .filter_map(|projection| match projection {
                    ast::Projection::Expr { expr, alias } => Some(alias.as_ref().map_or_else(
                        || self.text(expr.span).to_string(),
                        |alias| alias.node.clone(),
                    )),
                    _ => None,
                })
                .collect();
            for omitted in &select.omit {
                let name = self.text(omitted.span).to_string();
                let span = SourceSpan::new(self.ctx.source().clone(), omitted.span);
                let finding = if projected.contains(&name) {
                    surrealql_analyzer_diagnostics::catalog::finding(
                        span,
                        4012,
                        format!("`{name}` is projected and then omitted"),
                    )
                    .with_help(format!(
                        "drop `{name}` from the projection list instead of omitting it"
                    ))
                } else {
                    surrealql_analyzer_diagnostics::catalog::finding(
                        span,
                        4012,
                        format!(
                            "`OMIT {name}` has no effect: only the listed projections are returned, \
                             and `{name}` is not one of them"
                        ),
                    )
                    .with_help("OMIT removes fields from a `*` projection; with explicit projections, leave the field out of the list")
                };
                self.ctx.emit(finding);
            }
        }
        visit::walk_select(self, select);
    }
}
