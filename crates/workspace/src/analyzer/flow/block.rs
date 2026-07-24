//! Block analysis.
//!
//! A block's job is composition: each child statement is dispatched to that
//! statement's own analyzer, so per-statement rules stay attached to their
//! own analyzer. A block evaluates to its final statement's value.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::statement::analyze_lowered_statement;

pub(crate) fn analyze_block(ctx: &mut AnalysisContext<'_>, block: &ast::Block) -> Kind {
    let mut last = Kind::None;
    let mut terminated = false;
    for statement in &block.statements {
        if terminated {
            // Everything after a diverging statement never runs (4006) — one
            // finding at the first dead statement.
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
        // A guard that exits on its condition (`IF g THEN RETURN … END`) leaves
        // the negation of `g` holding for the statements that follow it.
        apply_fall_through_narrowing(ctx, &statement.node);
        terminated = statement_diverges(&statement.node);
    }
    last
}

/// After a preceding `IF` with no `ELSE` whose every branch diverges, the
/// statements that follow are only reached when every branch condition was
/// false — so their negations narrow the enclosing scope.
fn apply_fall_through_narrowing(ctx: &mut AnalysisContext<'_>, stmt: &ast::Statement) {
    let ast::Statement::IfElse(if_else) = stmt else {
        return;
    };
    if if_else.else_branch.is_some() || if_else.branches.is_empty() {
        return;
    }
    if !if_else.branches.iter().all(|branch| block_diverges(&branch.body)) {
        return;
    }
    let mut effects = Vec::new();
    for branch in &if_else.branches {
        effects.extend(crate::analyzer::flow::narrow::negative_effects(
            &branch.condition.node,
            ctx.env(),
        ));
    }
    crate::analyzer::flow::narrow::apply_effects(ctx, &effects);
}

/// Whether control flow provably cannot continue past this statement:
/// `RETURN`/`THROW`/`BREAK`/`CONTINUE` directly; an `IF`/`ELSE` in which the
/// `ELSE` is present and every branch (and the else) diverges; or a nested
/// block that itself diverges.
pub(crate) fn statement_diverges(stmt: &ast::Statement) -> bool {
    match stmt {
        ast::Statement::Return(_)
        | ast::Statement::Throw(_)
        | ast::Statement::Break(_)
        | ast::Statement::Continue(_) => true,
        ast::Statement::IfElse(if_else) => match &if_else.else_branch {
            Some(else_branch) => {
                !if_else.branches.is_empty()
                    && if_else
                        .branches
                        .iter()
                        .all(|branch| block_diverges(&branch.body))
                    && block_diverges(else_branch)
            }
            None => false,
        },
        ast::Statement::Block(block) => block_diverges(block),
        _ => false,
    }
}

/// Whether a block diverges: its last statement does.
pub(crate) fn block_diverges(block: &ast::Block) -> bool {
    block
        .statements
        .last()
        .is_some_and(|statement| statement_diverges(&statement.node))
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_diagnostics::Finding;
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
        let ast::Statement::Block(block) =
            surrealguard_syntax::lower::lower_first_statement(&parsed, "Block")
                .expect("no Block node in tree")
                .node
        else {
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

    /// The number of 4006 findings raised for `source` analyzed as a workspace.
    fn unreachable_count(source: &str) -> usize {
        let mut workspace = crate::analysis::Workspace::default();
        workspace.add_virtual_source("a1-e2e".into(), source.into());
        crate::analysis::analyze_workspace(&workspace)
            .diagnostics
            .iter()
            .filter(|finding| finding.code().number() == 4006)
            .count()
    }

    #[test]
    fn unreachable_after_an_all_branches_diverging_if_else() {
        // Both branches (and the else) RETURN, so `RETURN 3` is dead.
        let source = "DEFINE FUNCTION fn::f($x: int) -> int {\n\
             IF $x > 0 { RETURN 1; } ELSE { RETURN 2; };\n\
             RETURN 3;\n\
         };";
        assert_eq!(unreachable_count(source), 1);
    }

    #[test]
    fn unreachable_after_a_diverging_nested_block() {
        // A nested block that itself diverges kills the following statement.
        let source = "DEFINE FUNCTION fn::f() {\n\
             { RETURN 1; };\n\
             RETURN 2;\n\
         };";
        assert_eq!(unreachable_count(source), 1);
    }

    #[test]
    fn direct_return_run_still_flags_the_second_return() {
        // The pre-existing flat-terminator behavior is preserved.
        let source = "DEFINE FUNCTION fn::f() { RETURN 1; RETURN 2; };";
        assert_eq!(unreachable_count(source), 1);
    }

    #[test]
    fn if_without_an_else_is_not_unreachable() {
        // Control can fall through an `IF` that has no `ELSE`.
        let source = "DEFINE FUNCTION fn::f($x: int) {\n\
             IF $x > 0 { RETURN 1; };\n\
             RETURN 2;\n\
         };";
        assert_eq!(unreachable_count(source), 0);
    }

    #[test]
    fn one_branch_falling_through_is_not_unreachable() {
        // When a branch does not diverge, the following statement is reachable.
        let source = "DEFINE FUNCTION fn::f($x: int) {\n\
             IF $x > 0 { RETURN 1; } ELSE { LET $y = 2; };\n\
             RETURN 3;\n\
         };";
        assert_eq!(unreachable_count(source), 0);
    }
}
