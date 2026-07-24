//! `FOR` statement analysis.
//!
//! The loop binding is defined in the body's child scope with the element
//! kind of the iterable when it is known. The statement produces no value.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::expression::{ExpressionFact, ExpressionValueClass};

pub(crate) fn analyze_for_loop(ctx: &mut AnalysisContext<'_>, stmt: &ast::ForStmt) -> Kind {
    let iterable = crate::analyzer::expression::expr_fact(ctx, &stmt.iterable);
    let element_kind = match &iterable.kind {
        Some(Kind::Array(element, _) | Kind::Set(element, _)) => Some((**element).clone()),
        _ => None,
    };

    // FOR's contract: the iterable is a collection (or a range, once those
    // are modeled). Definitely-scalar kinds are 2022.
    if let Some(kind) = &iterable.kind {
        let base = crate::kinds::literal_base_kind(kind).unwrap_or_else(|| kind.clone());
        if matches!(
            base,
            Kind::Int
                | Kind::Float
                | Kind::Decimal
                | Kind::Number
                | Kind::Bool
                | Kind::String
                | Kind::Datetime
                | Kind::Duration
                | Kind::Uuid
                | Kind::None
                | Kind::Null
        ) {
            let span = surrealguard_syntax::span::SourceSpan::new(
                ctx.source().clone(),
                stmt.iterable.span,
            );
            ctx.emit(surrealguard_diagnostics::catalog::finding(
                span,
                2022,
                format!("FOR can't iterate a `{}` — it is not a collection", crate::render_kind(kind)),
            ));
        }
        // Constant empty collections never run their body (7004: control
        // flow decided by a constant).
        if let Some(surrealdb_types::Value::Array(values)) = &iterable.value {
            if values.is_empty() {
                let span = surrealguard_syntax::span::SourceSpan::new(
                    ctx.source().clone(),
                    stmt.iterable.span,
                );
                ctx.emit(
                    surrealguard_diagnostics::catalog::finding(
                        span,
                        7004,
                        "this loop never runs: the collection is a constant empty array"
                            .to_string(),
                    )
                    .with_help("remove the loop, or iterate a non-empty collection"),
                );
            }
        }
    }

    ctx.with_child_env(|ctx| {
        let mut binding =
            ExpressionFact::new(iterable.span.clone(), ExpressionValueClass::Variable);
        binding.kind = element_kind.clone();
        // Record the loop variable (element kind of the iterated collection)
        // for editor features, so `$parent` in `FOR $parent IN ...` hovers
        // and gets an inlay hint the same as a LET.
        let name_span = surrealguard_syntax::span::SourceSpan::new(
            ctx.source().clone(),
            stmt.binding.span,
        );
        ctx.record_let_binding(crate::analysis::LetBindingAnalysis {
            name: stmt.binding.node.clone(),
            name_span,
            kind: element_kind,
        });
        ctx.define_local(stmt.binding.node.clone(), binding);
        ctx.with_loop(|ctx| crate::analyzer::flow::block::analyze_block(ctx, &stmt.body))
    });

    Kind::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_diagnostics::Finding;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    use crate::schema::SchemaIndex;

    #[test]
    fn loops_produce_no_value_and_do_not_leak_the_binding() {
        let parsed = parse_source(
            SourceId::new("flow:test"),
            "FOR $item IN [1, 2] { RETURN $item; };",
        )
        .expect("query parses");
        let ast::Statement::For(stmt) =
            surrealguard_syntax::lower::lower_first_statement(&parsed, "ForStatement")
                .expect("no ForStatement node in tree")
                .node
        else {
            panic!("expected for statement");
        };
        let schema = SchemaIndex::default();
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(
            &schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
        );

        let kind = analyze_for_loop(&mut ctx, &stmt);

        assert_eq!(kind, Kind::None);
        assert!(ctx.env().let_fact("item").is_none());
    }
}
