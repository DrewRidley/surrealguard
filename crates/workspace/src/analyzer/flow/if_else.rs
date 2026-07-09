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
        // not their value — but a condition whose kind can never be a bool
        // is worth a warning (truthiness makes it run regardless).
        let condition_fact = crate::analyzer::expression::expr_fact(ctx, &branch.condition);
        if condition_fact.value.is_some() {
            let span = surrealguard_syntax::span::SourceSpan::new(
                ctx.source().clone(),
                branch.condition.span,
            );
            ctx.emit(surrealguard_diagnostics::catalog::finding(
                span,
                7004,
                "condition is constant".to_string(),
            ));
        }
        let condition_kind = condition_fact.kind.unwrap_or(Kind::Any);
        if definitely_not_bool(&condition_kind) {
            let span = surrealguard_syntax::span::SourceSpan::new(
                ctx.source().clone(),
                branch.condition.span,
            );
            ctx.emit(surrealguard_diagnostics::catalog::finding(
                span,
                2005,
                format!("IF condition has type `{condition_kind}`, expected `bool`"),
            ));
        }
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

/// Whether a condition of this kind can never evaluate to a boolean.
/// Unknowns, unions containing bool, and NONE/NULL (falsy) are all fine.
pub(crate) fn definitely_not_bool(kind: &Kind) -> bool {
    match kind {
        Kind::Bool | Kind::Any | Kind::None | Kind::Null => false,
        Kind::Either(variants) => variants.iter().all(definitely_not_bool),
        Kind::Literal(literal) => !matches!(literal, surrealdb_types::KindLiteral::Bool(_)),
        _ => true,
    }
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
