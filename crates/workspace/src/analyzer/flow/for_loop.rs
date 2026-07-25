//! `FOR` statement analysis.
//!
//! The loop binding is defined in the body's child scope with the element
//! kind of the iterable when it is known. The statement itself produces no
//! value (a `FOR` evaluates to `NONE` and never diverges — the body may run
//! zero times), but any `RETURN` reachable inside the body is an exit of the
//! enclosing function/closure and bubbles up through its [`Flow`].

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::flow::block::{analyze_block_flow, Flow};
use crate::expression::{ExpressionFact, ExpressionValueClass};

/// A `FOR` as a statement produces no value; its body's `RETURN` exits are
/// collected for the enclosing exit set by [`analyze_for_loop_flow`] and
/// discarded here (a bare `FOR` at statement position has no exit set to
/// bubble into).
pub(crate) fn analyze_for_loop(ctx: &mut AnalysisContext<'_>, stmt: &ast::ForStmt) -> Kind {
    analyze_for_loop_flow(ctx, stmt);
    Kind::None
}

/// The [`Flow`] of a `FOR`: the `RETURN`s reachable inside its body (in the
/// loop/element env), no pass-through value (`NONE`), and never diverging.
pub(crate) fn analyze_for_loop_flow(ctx: &mut AnalysisContext<'_>, stmt: &ast::ForStmt) -> Flow {
    let iterable = crate::analyzer::expression::expr_fact(ctx, &stmt.iterable);
    // The loop binding's kind, distributed over a union: an
    // `option<array<T>>` is `Either([None, Array(T)])` and a flat
    // `Array | Set` match sees only the `Either`, leaving the binding
    // untyped — so nothing in the body gets checked against it.
    let element_kind = iterable
        .kind
        .as_ref()
        .and_then(crate::analyzer::expression::infer::collection_element_kind);

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

    let body = ctx.with_child_env(|ctx| {
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
        ctx.with_loop(|ctx| analyze_block_flow(ctx, &stmt.body))
    });

    // The body's `RETURN`s exit the enclosing function; the `FOR` itself
    // yields `NONE` and never diverges (it may iterate zero times).
    Flow {
        returns: body.returns,
        value: Kind::None,
        diverges: false,
    }
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

    /// NEW-4: an `option<array<T>>` is `Either([None, Array(T)])`. A flat
    /// `Array | Set` match left the loop binding untyped, so nothing in the
    /// body was checked against it.
    #[test]
    fn a_union_wrapped_iterable_still_types_the_loop_binding() {
        use crate::analysis::{analyze_workspace, Workspace};

        let schema_text = "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD tags ON t TYPE option<array<string>>;\n\
             DEFINE FIELD plain ON t TYPE array<string>;\n\
             DEFINE FIELD label ON t TYPE option<string>;";

        let bogus_method = |query: &str| {
            let mut workspace = Workspace::default();
            workspace.add_virtual_source("schema".into(), schema_text.into());
            let source = workspace.add_virtual_source("query".into(), query.into());
            let output = analyze_workspace(&workspace);
            output.sources[&source]
                .diagnostics
                .iter()
                .any(|finding| finding.code().number() == 5001)
        };

        // Control: a plain `array<string>` types the binding as `string`, so a
        // bogus method on it is 5001.
        assert!(bogus_method(
            "FOR $x IN (SELECT VALUE plain FROM ONLY t LIMIT 1) { RETURN $x.not_a_real_method(); };"
        ));
        // The regression: the `option<array<string>>` form must type it too.
        assert!(bogus_method(
            "FOR $x IN (SELECT VALUE tags FROM ONLY t LIMIT 1) { RETURN $x.not_a_real_method(); };"
        ));
        // Negative: no arm of the iterable is a collection, so no element kind
        // is invented and the body stays unchecked rather than guessed at.
        assert!(!bogus_method(
            "FOR $x IN (SELECT VALUE label FROM ONLY t LIMIT 1) { RETURN $x.not_a_real_method(); };"
        ));
    }
}
