//! `IF`/`ELSE` statement analysis.
//!
//! Each branch body is analyzed in its own child scope (branch-local `LET`
//! bindings don't leak) under the branch condition's flow narrowing (positive
//! effects in a THEN, the negation of every branch in the ELSE).
//!
//! An `IF` is itself a value-block-like construct, so it is typed by the same
//! exit-set model as a block ([`crate::analyzer::flow::block::Flow`]): each
//! branch's `RETURN`s bubble out as exits, and its *pass-through* value (the
//! trailing value of a branch that does not diverge, or `NONE` for the
//! implicit fall-through of an `IF` with no `ELSE`) contributes to the value
//! reached when control continues past the `IF`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::flow::block::{analyze_block_flow, Flow};

/// The value of an `IF` as a statement: its exit-set union (every branch's
/// `RETURN` exits, plus the pass-through value when it does not diverge).
pub(crate) fn analyze_if_else(ctx: &mut AnalysisContext<'_>, stmt: &ast::IfElseStmt) -> Kind {
    analyze_if_else_flow(ctx, stmt).into_kind()
}

/// The [`Flow`] of an `IF`/`ELSE`: the `RETURN`s reachable through any branch,
/// the value reached when control falls past the `IF`, and whether it
/// provably diverges (only when an `ELSE` is present and every branch and the
/// `ELSE` diverge).
pub(crate) fn analyze_if_else_flow(ctx: &mut AnalysisContext<'_>, stmt: &ast::IfElseStmt) -> Flow {
    let mut returns: Vec<Kind> = Vec::new();
    // Pass-through values: the trailing value of each branch through which
    // control can continue past the `IF`.
    let mut pass_through: Vec<Kind> = Vec::new();
    // Whether *every* arm (branches and the `ELSE`, if any) diverges.
    let mut all_arms_diverge = true;

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
                "this IF condition is constant, so one branch is never taken".to_string(),
            ));
        }
        let condition_kind = condition_fact.kind.unwrap_or(Kind::Any);
        if definitely_not_bool(&condition_kind) {
            let span = surrealguard_syntax::span::SourceSpan::new(
                ctx.source().clone(),
                branch.condition.span,
            );
            ctx.emit(
                surrealguard_diagnostics::catalog::finding(
                    span,
                    2005,
                    format!("this IF condition is a `{}`, not a `bool`", crate::render_kind(&condition_kind)),
                )
                .with_help("an IF chooses a branch on a true/false test; the condition must be a bool"),
            );
        }
        // The THEN body runs only when the branch condition holds, so it
        // sees the positive narrowing of that condition's guards.
        let positive =
            crate::analyzer::flow::narrow::positive_effects(&branch.condition.node, ctx.env());
        let flow = ctx.with_child_env(|ctx| {
            crate::analyzer::flow::narrow::apply_effects(ctx, &positive);
            analyze_block_flow(ctx, &branch.body)
        });
        returns.extend(flow.returns);
        if !flow.diverges {
            all_arms_diverge = false;
            pass_through.push(flow.value);
        }
    }

    if let Some(else_branch) = &stmt.else_branch {
        // The ELSE runs only when every preceding branch condition was false,
        // so it sees the negation of each.
        let mut negative = Vec::new();
        for branch in &stmt.branches {
            negative.extend(crate::analyzer::flow::narrow::negative_effects(
                &branch.condition.node,
                ctx.env(),
            ));
        }
        let flow = ctx.with_child_env(|ctx| {
            crate::analyzer::flow::narrow::apply_effects(ctx, &negative);
            analyze_block_flow(ctx, else_branch)
        });
        returns.extend(flow.returns);
        if !flow.diverges {
            all_arms_diverge = false;
            pass_through.push(flow.value);
        }
    } else {
        // With no ELSE, a non-matching `IF` falls through to `NONE` at runtime,
        // so `NONE` is always a reachable pass-through value.
        pass_through.push(Kind::None);
    }

    // The `IF` diverges only when an `ELSE` is present and every arm diverges —
    // otherwise control can fall through (mirrors `block::statement_diverges`).
    let diverges = stmt.else_branch.is_some() && !stmt.branches.is_empty() && all_arms_diverge;

    Flow {
        returns,
        value: Kind::either(pass_through),
        diverges,
    }
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
        let ast::Statement::IfElse(stmt) =
            surrealguard_syntax::lower::lower_first_statement(&parsed, "IfElseStatement")
                .expect("no IfElseStatement node in tree")
                .node
        else {
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

    /// The kind of the first `IfElseStatement` in `source`.
    fn if_else_kind(source: &str) -> Kind {
        let parsed = parse_source(SourceId::new("flow:test"), source).expect("query parses");
        let ast::Statement::IfElse(stmt) =
            surrealguard_syntax::lower::lower_first_statement(&parsed, "IfElseStatement")
                .expect("no IfElseStatement node in tree")
                .node
        else {
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
        analyze_if_else(&mut ctx, &stmt)
    }

    #[test]
    fn if_without_an_else_includes_the_implicit_none_fall_through() {
        // A value-producing branch plus the no-ELSE fall-through: `int | none`.
        assert_eq!(
            if_else_kind("IF $c THEN 1 END;"),
            Kind::Either(vec![Kind::Int, Kind::None])
        );
    }

    #[test]
    fn diverging_branch_without_an_else_surfaces_return_and_none() {
        // The THEN diverges (RETURN 1) but the no-ELSE fall-through still
        // evaluates to NONE — the IF's value is `int | none`, and the `1`
        // reaches the enclosing exit set as a RETURN.
        assert_eq!(
            if_else_kind("IF $c THEN RETURN 1 END;"),
            Kind::Either(vec![Kind::Int, Kind::None])
        );
    }
}
