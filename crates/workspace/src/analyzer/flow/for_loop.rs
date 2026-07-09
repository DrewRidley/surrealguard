//! `FOR` statement analysis.
//!
//! The loop binding is defined in the body's child scope with the element
//! kind of the iterable when it is known. The statement produces no value.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::expression::{ExpressionFact, ExpressionValueClass};

pub fn analyze_for_loop(ctx: &mut AnalysisContext<'_>, stmt: &ast::ForStmt) -> Kind {
    let iterable = crate::analyzer::expression::expr_fact(ctx, &stmt.iterable);
    let element_kind = match iterable.kind {
        Some(Kind::Array(element, _)) | Some(Kind::Set(element, _)) => Some(*element),
        _ => None,
    };

    ctx.with_child_env(|ctx| {
        let mut binding =
            ExpressionFact::new(iterable.span.clone(), ExpressionValueClass::Variable);
        binding.kind = element_kind;
        ctx.define_local(stmt.binding.node.clone(), binding);
        ctx.with_loop(|ctx| crate::analyzer::flow::block::analyze_block(ctx, &stmt.body))
    });

    Kind::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_diagnostics::Finding;
    use surrealguard_syntax::lower::lower_statement;
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
        let node =
            crate::analyzer::flow::tests::first_of_kind(parsed.tree().root_node(), "ForStatement");
        let ast::Statement::For(stmt) = lower_statement(node, parsed.text()).node else {
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
