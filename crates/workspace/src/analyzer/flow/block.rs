//! Block analysis.
//!
//! A block's job is composition: each child statement is dispatched to that
//! statement's own analyzer, so per-statement rules stay attached to their
//! own analyzer. A block evaluates to its final statement's value.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::statement::analyze_lowered_statement;

pub fn analyze_block(ctx: &mut AnalysisContext<'_>, block: &ast::Block) -> Kind {
    let mut last = Kind::None;
    let mut terminated = false;
    for statement in &block.statements {
        if terminated {
            // Everything after a RETURN/BREAK/CONTINUE/THROW never runs
            // (4006) — one finding at the first dead statement.
            let span =
                surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), statement.span);
            ctx.emit(
                surrealguard_diagnostics::catalog::finding(
                    span,
                    4006,
                    "unreachable: the block already returned".to_string(),
                )
                .with_tag(surrealguard_diagnostics::FindingTag::Unnecessary),
            );
            break;
        }
        last = analyze_lowered_statement(ctx, statement);
        terminated = matches!(
            statement.node,
            ast::Statement::Return(_)
                | ast::Statement::Break(_)
                | ast::Statement::Continue(_)
                | ast::Statement::Throw(_)
        );
    }
    last
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
    fn evaluates_to_the_final_statement_value_with_let_flow() {
        let parsed = parse_source(
            SourceId::new("flow:test"),
            "RETURN { LET $x = 1; RETURN $x + 1; };",
        )
        .expect("query parses");
        let node = crate::analyzer::flow::tests::first_of_kind(parsed.tree().root_node(), "Block");
        let ast::Statement::Block(block) = lower_statement(node, parsed.text()).node else {
            panic!("expected block");
        };
        let schema = SchemaIndex::default();
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(
            &schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
        );

        let kind = analyze_block(&mut ctx, &block);

        assert_eq!(kind, Kind::Int);
    }
}
