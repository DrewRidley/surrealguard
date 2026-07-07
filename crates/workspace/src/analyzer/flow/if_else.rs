//! `IF`/`ELSE` statement analysis.
//!
//! Each branch body is analyzed in its own child scope (branch-local `LET`
//! bindings don't leak); the statement's value is the union of the branch
//! values, merged through upstream `Kind::either`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_if_else(ctx: &mut AnalysisContext<'_>, stmt: &ast::IfElseStmt) -> Kind {
    let mut branch_kinds = Vec::new();

    for branch in &stmt.branches {
        // Conditions are analyzed for their facts (params, dependencies),
        // not their value.
        let _ = crate::analyzer::expression::analyze_expr(ctx, &branch.condition);
        let kind = ctx
            .with_child_env(|ctx| crate::analyzer::flow::block::analyze_block(ctx, &branch.body));
        branch_kinds.push(kind);
    }
    if let Some(else_branch) = &stmt.else_branch {
        let kind =
            ctx.with_child_env(|ctx| crate::analyzer::flow::block::analyze_block(ctx, else_branch));
        branch_kinds.push(kind);
    }

    if branch_kinds.is_empty() {
        return Kind::Any;
    }
    Kind::either(branch_kinds)
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
    fn merges_branch_values_and_scopes_branch_local_lets() {
        let parsed = parse_source(
            SourceId::new("flow:test"),
            "IF true { LET $v = 1; RETURN $v; } ELSE { RETURN 's'; };",
        )
        .expect("query parses");
        let node = crate::analyzer::flow::tests::first_of_kind(
            parsed.tree().root_node(),
            "IfElseStatement",
        );
        let ast::Statement::IfElse(stmt) = lower_statement(node, parsed.text()).node else {
            panic!("expected if statement");
        };
        let schema = SchemaIndex::default();
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(
            &schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
        );

        let kind = analyze_if_else(&mut ctx, &stmt);

        assert_eq!(kind, Kind::Either(vec![Kind::Int, Kind::String]));
        // The branch-local LET must not leak into the outer scope.
        assert!(ctx.env().let_fact("v").is_none());
    }
}
