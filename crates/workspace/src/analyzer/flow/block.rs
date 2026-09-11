//! Block analysis.
//!
//! A block's job is composition: each child statement is dispatched to that
//! statement's own analyzer, so per-statement rules stay attached to their
//! own analyzer.
//!
//! A block is typed by its **exit set**: the union over every reachable
//! control-flow exit. There are two kinds of exit:
//!
//! 1. a `RETURN e`, contributing the kind of `e` in the flow-narrowed env
//!    that holds on the path to it (non-trailing `RETURN`s inside `IF`/`FOR`/
//!    nested blocks are collected too, not just the trailing statement); and
//! 2. the trailing statement's value, but **only when control can reach the
//!    end of the block** — a block that provably diverges contributes no
//!    trailing value.
//!
//! No-value diverging exits (`THROW`/`BREAK`/`CONTINUE`) contribute nothing.
//! The union is the lattice join ([`crate::lattice::join_all`]), which
//! flattens, dedupes, collapses a singleton, lets `any` absorb, and yields
//! `Kind::None` for the empty set. The exit set over-approximates the runtime
//! value set: widening (an extra exit, an extra `None`) is sound; dropping a
//! reachable exit is the failure mode.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;
use surrealql_analyzer_syntax::span::ByteRange;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::statement::analyze_lowered_statement;

/// The control-flow summary of a block or block-like construct: the value
/// kinds carried by every `RETURN` reachable inside it, the value it
/// evaluates to when control reaches its end, and whether control provably
/// cannot reach that end.
///
/// `returns` bubble up to the enclosing function/closure/value-block exit set;
/// `value` is the trailing-position contribution, used only when `!diverges`.
pub(crate) struct Flow {
    /// Kinds carried out by each reachable `RETURN` (already flow-narrowed).
    pub returns: Vec<Kind>,
    /// The value produced when control reaches the end of the construct.
    pub value: Kind,
    /// Whether control provably cannot reach the end.
    pub diverges: bool,
}

impl Flow {
    /// The exit-set union: every `RETURN` exit, plus the trailing value when
    /// the construct does not provably diverge. `join_all([]) == Kind::None`.
    ///
    /// `Kind::Any` is the top type, so a union that includes it *is* `Any` —
    /// any exit typed `Any` (e.g. `record::id(...)`, a recursive UDF call)
    /// absorbs the whole union. That is the join's own rule; it keeps `T | any`
    /// from leaking out (which would, for instance, defeat the mutation
    /// checker's `Any` short-circuit) and is the sound over-approximation.
    pub(crate) fn into_kind(self) -> Kind {
        let mut exits = self.returns;
        if !self.diverges {
            exits.push(self.value);
        }
        // An empty-array literal (`array<_, 0>` — only the empty array) is a
        // member of every `array<E>`/`set<E>`, so when a concrete collection
        // exit is present the empty literal contributes no new inhabitant. Drop
        // it, so a guard's `RETURN []` (e.g. `IF array::len($rows) = 0 THEN
        // RETURN [] END; … RETURN $mapped`) yields `array<E>` rather than
        // `array<any,0> | array<E>`.
        let is_empty_collection =
            |k: &Kind| matches!(k, Kind::Array(_, Some(0)) | Kind::Set(_, Some(0)));
        let has_concrete_collection = exits
            .iter()
            .any(|k| matches!(k, Kind::Array(_, _) | Kind::Set(_, _)) && !is_empty_collection(k));
        if has_concrete_collection {
            exits.retain(|k| !is_empty_collection(k));
        }
        crate::lattice::join_all(exits)
    }
}

/// A block's value is its exit-set union (see the module docs).
pub(crate) fn analyze_block(ctx: &mut AnalysisContext<'_>, block: &ast::Block) -> Kind {
    analyze_block_flow(ctx, block).into_kind()
}

/// Analyzes each statement in order and accumulates the block's [`Flow`]: the
/// `RETURN` exits collected from every statement (each in the env in force at
/// that point), the trailing statement's value, and whether the block
/// diverges. This is the single analysis pass — every statement's own
/// analyzer runs exactly once here.
pub(crate) fn analyze_block_flow(ctx: &mut AnalysisContext<'_>, block: &ast::Block) -> Flow {
    // A fall-through narrowing established inside this block dies with the
    // block, so the statement sequence's end bounds the region it covers.
    let block_end = block
        .statements
        .last()
        .map(|statement| statement.span.end());
    ctx.with_scope_end(block_end, |ctx| block_flow(ctx, block))
}

fn block_flow(ctx: &mut AnalysisContext<'_>, block: &ast::Block) -> Flow {
    let mut returns: Vec<Kind> = Vec::new();
    let mut value = Kind::None;
    let mut terminated = false;
    for statement in &block.statements {
        if terminated {
            // Everything after a diverging statement never runs (4006) — one
            // finding at the first dead statement.
            let span = surrealql_analyzer_syntax::span::SourceSpan::new(
                ctx.source().clone(),
                statement.span,
            );
            ctx.emit(
                surrealql_analyzer_diagnostics::catalog::finding(
                    span,
                    4006,
                    "this statement is unreachable — the block already returned".to_string(),
                )
                .with_tag(surrealql_analyzer_diagnostics::FindingTag::Unnecessary),
            );
            break;
        }
        let stmt_flow = statement_flow(ctx, statement);
        returns.extend(stmt_flow.returns);
        value = stmt_flow.value;
        // A guard that exits on its condition (`IF g THEN RETURN … END`) leaves
        // the negation of `g` holding for the statements that follow it.
        apply_fall_through_narrowing(ctx, statement);
        // A block terminates on unambiguous divergence only: syntactic
        // (RETURN/THROW/BREAK/CONTINUE, or an else-covered IF whose every arm
        // diverges) OR constant (a compile-time-true guard with no reachable
        // fall-through whose body diverges — `IF 1 == 1 { RETURN … }`). Dead code
        // reachable only by TYPE-NARROWING exhaustiveness (a defensive final
        // `RETURN` after branches that cover a record's whole table union) is
        // deliberately NOT terminated here: it stays a live exit, neither dropped
        // from the return type nor flagged 4006.
        terminated = statement_diverges(&statement.node) || const_diverges(&statement.node);
    }
    Flow {
        returns,
        value,
        diverges: terminated,
    }
}

/// The `RETURN` exits carried out of a single statement, plus the value it
/// evaluates to. Composite statements (`IF`/`FOR`/nested blocks) surface the
/// `RETURN`s nested inside them so an early exit is never dropped; every other
/// statement dispatches to its own analyzer and carries no `RETURN` exit.
fn statement_flow(ctx: &mut AnalysisContext<'_>, statement: &ast::Spanned<ast::Statement>) -> Flow {
    match &statement.node {
        // A `RETURN` is itself an exit: its value both bubbles up as a return
        // and is the statement's value (moot — a `RETURN` diverges).
        ast::Statement::Return(stmt) => {
            let kind = crate::analyzer::flow::return_stmt::analyze_return(ctx, stmt);
            Flow {
                returns: vec![kind.clone()],
                value: kind,
                diverges: true,
            }
        }
        // Composite constructs surface their nested `RETURN`s and their own
        // pass-through value (see the respective flow builders).
        ast::Statement::IfElse(stmt) => {
            crate::analyzer::flow::if_else::analyze_if_else_flow(ctx, stmt)
        }
        ast::Statement::For(stmt) => {
            crate::analyzer::flow::for_loop::analyze_for_loop_flow(ctx, stmt)
        }
        ast::Statement::Block(block) => analyze_block_flow(ctx, block),
        // Everything else carries no `RETURN` exit; its value is the trailing
        // contribution. `THROW`/`BREAK`/`CONTINUE` divergence is recognized by
        // `statement_diverges`, which drops the (no-)value at the block level.
        _ => {
            let value = analyze_lowered_statement(ctx, statement);
            Flow {
                returns: Vec::new(),
                value,
                diverges: false,
            }
        }
    }
}

/// After a preceding `IF` with no `ELSE` whose every branch diverges, the
/// statements that follow are only reached when every branch condition was
/// false — so their negations narrow the enclosing scope.
///
/// Applies at every statement-sequence level: inside a block (here) and at a
/// source's top level (the pipeline's statement loop), which is where the
/// idiomatic `LET $x = SELECT … FROM ONLY …; IF $x = NONE THEN THROW … END;`
/// guard lives.
pub(crate) fn apply_fall_through_narrowing(
    ctx: &mut AnalysisContext<'_>,
    statement: &ast::Spanned<ast::Statement>,
) {
    let ast::Statement::IfElse(if_else) = &statement.node else {
        return;
    };
    let guard_span = statement.span;
    if if_else.else_branch.is_some() || if_else.branches.is_empty() {
        return;
    }
    if !if_else
        .branches
        .iter()
        .all(|branch| block_diverges(&branch.body))
    {
        return;
    }
    // The refinement holds from just past the guard to the end of the
    // statement sequence the guard sits in — the guard itself, and everything
    // before it, still reads the declared kind.
    let region = ByteRange::new(guard_span.end(), ctx.scope_end().max(guard_span.end())).ok();
    // Reaching here means every branch condition was false — the same region an
    // `ELSE` on the same `IF` would name, read by the same function.
    crate::analyzer::flow::narrow::narrow_where_all_false(
        ctx,
        if_else.branches.iter().map(|branch| &branch.condition.node),
        region,
    );
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
        // A statement that is just an expression diverges when the expression
        // does. `{ … }` and `(…)` and `IF … { RETURN 1 } ELSE { RETURN 2 }` in
        // value position all lower to this shape, and all three were invisible
        // here — so a block ending in one was not a diverging block, and the
        // fall-through narrowing an enclosing guard should have established
        // died with it.
        ast::Statement::Expr(expr) => expr_diverges(&expr.node),
        _ => false,
    }
}

/// Whether an expression, evaluated for its value, provably never yields one.
///
/// Only the forms that *contain statements* can: a block, a parenthesized
/// statement, an `IF` used as a value. Everything else is a value computation,
/// which either produces a value or raises at runtime — and a runtime raise is
/// not something this layer proves.
fn expr_diverges(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Block(block) => block_diverges(block),
        ast::Expr::Subquery(inner) => statement_diverges(&inner.node),
        _ => false,
    }
}

/// Whether a statement provably diverges by CONSTANT reasoning alone: an `IF`
/// whose guard is a compile-time constant (`IF 1 == 1 { RETURN … }`) that makes
/// the `ELSE`/fall-through dead and whose every still-reachable branch body
/// diverges. Uses the const-only [`branch_reachability`], never env narrowing.
///
/// Deliberately const-only: code proven unreachable *only* by type narrowing (a
/// defensive `RETURN false` after branches that exhaustively cover a record's
/// table union) is idiomatic and stays live — it is neither dropped from the
/// exit-set type nor flagged 4006. Only unambiguous constant divergence (this)
/// and syntactic divergence ([`statement_diverges`]) terminate a block.
///
/// [`branch_reachability`]: crate::analyzer::const_eval::branch_reachability
fn const_diverges(stmt: &ast::Statement) -> bool {
    let ast::Statement::IfElse(if_else) = stmt else {
        return false;
    };
    let reach = crate::analyzer::const_eval::branch_reachability(if_else);
    // `else_dead` means a compile-time-true branch is guaranteed to run, so there
    // is no fall-through past the `IF`; it diverges iff every branch that can
    // still run diverges.
    reach.else_dead
        && if_else
            .branches
            .iter()
            .zip(&reach.branches)
            .filter(|(_, branch_reach)| branch_reach.is_reachable())
            .all(|(branch, _)| block_diverges(&branch.body))
}

/// Whether a block diverges: **any** statement in it does.
///
/// Control cannot reach the end of a sequence once it cannot get past one of
/// its statements, so the position of the diverging statement is irrelevant —
/// `{ THROW 'e'; RETURN 1; }` diverges exactly as `{ RETURN 1; }` does. Reading
/// only the last statement made the answer depend on whether dead code had been
/// written after the exit, which is the one thing it cannot depend on.
///
/// The rule is the same one [`block_flow`] terminates on, and now it is
/// literally the same pair of predicates: a block whose `IF 1 = 1 { RETURN … }`
/// made `block_flow` stop was, until now, not a diverging block to anybody
/// asking from outside.
pub(crate) fn block_diverges(block: &ast::Block) -> bool {
    block
        .statements
        .iter()
        .any(|statement| statement_diverges(&statement.node) || const_diverges(&statement.node))
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealql_analyzer_diagnostics::Finding;
    use surrealql_analyzer_syntax::parse::parse_source;
    use surrealql_analyzer_syntax::source::SourceId;

    use crate::schema::SchemaIndex;

    /// Lowers `source`'s first `Block` node, runs `bind` to seed the env, and
    /// returns the block's exit-set kind.
    fn block_exit_kind(source: &str, bind: impl FnOnce(&mut AnalysisContext<'_>)) -> Kind {
        let parsed = parse_source(SourceId::new("flow:test"), source).expect("query parses");
        let ast::Statement::Block(block) =
            surrealql_analyzer_syntax::lower::lower_first_statement(&parsed, "Block")
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
        bind(&mut ctx);
        analyze_block(&mut ctx, &block)
    }

    /// Binds `$name` to `kind` as a local in `ctx`.
    fn bind_param(ctx: &mut AnalysisContext<'_>, name: &str, kind: Kind) {
        use crate::expression::{ExpressionFact, ExpressionValueClass};
        let span = surrealql_analyzer_syntax::span::SourceSpan::new(
            ctx.source().clone(),
            surrealql_analyzer_syntax::span::ByteRange::new(0, 1).unwrap(),
        );
        let fact = ExpressionFact::new(span, ExpressionValueClass::Variable).with_kind(kind);
        ctx.define_local(name.to_string(), fact);
    }

    #[test]
    fn non_trailing_return_widens_the_block_to_the_exit_union() {
        // The canonical bug (design §3.3): `$x` is `option<{ name: string }>`.
        // The early `RETURN false` (in the NONE guard) must not be dropped —
        // the block's honest type is `bool | string`, not just the trailing
        // `string`. And the trailing `RETURN $x.name` is typed in the
        // fall-through-narrowed env where `$x` is non-none, so `.name` resolves.
        let x_kind = Kind::either(vec![
            Kind::None,
            Kind::Literal(surrealdb_types::KindLiteral::Object(
                std::collections::BTreeMap::from([("name".to_string(), Kind::String)]),
            )),
        ]);
        let kind = block_exit_kind(
            "RETURN { IF $x = NONE THEN RETURN false END; RETURN $x.name };",
            |ctx| bind_param(ctx, "x", x_kind.clone()),
        );
        assert_eq!(kind, Kind::Either(vec![Kind::Bool, Kind::String]));
    }

    #[test]
    fn early_return_unions_with_the_trailing_return() {
        // `IF c THEN RETURN 1 END; RETURN 'x'` → `int | string`. The non-trailing
        // RETURN is captured; the IF's own NONE fall-through is not spuriously
        // added (the trailing statement diverges).
        let kind = block_exit_kind("RETURN { IF $c THEN RETURN 1 END; RETURN 'x' };", |_ctx| {});
        assert_eq!(kind, Kind::Either(vec![Kind::Int, Kind::String]));
    }

    #[test]
    fn if_without_else_contributes_an_implicit_none_to_the_block_value() {
        // A plain `IF c THEN 1 END` as a block's trailing value can fall
        // through to NONE, so the block value is `int | none`.
        let kind = block_exit_kind("RETURN { IF $c THEN 1 END };", |_ctx| {});
        assert_eq!(kind, Kind::Either(vec![Kind::Int, Kind::None]));
    }

    #[test]
    fn a_fully_diverging_block_has_no_trailing_none_contribution() {
        // Every path RETURNs, so the block diverges: the exit set is exactly
        // the two RETURN kinds — no trailing NONE is added.
        let kind = block_exit_kind(
            "RETURN { IF $c THEN RETURN 1 ELSE RETURN 'x' END };",
            |_ctx| {},
        );
        assert_eq!(kind, Kind::Either(vec![Kind::Int, Kind::String]));
    }

    #[test]
    fn returns_nested_in_a_child_block_bubble_to_the_exit_set() {
        // A RETURN inside a nested block is an exit of the enclosing block too.
        let kind = block_exit_kind(
            "RETURN { IF $c THEN RETURN 1 END; { RETURN 'nested' } };",
            |_ctx| {},
        );
        assert_eq!(kind, Kind::Either(vec![Kind::Int, Kind::String]));
    }

    #[test]
    fn returns_inside_a_for_body_bubble_to_the_exit_set() {
        // A loop may return an element or fall through to the trailing RETURN.
        let kind = block_exit_kind(
            "RETURN { FOR $i IN [1, 2] { RETURN $i; }; RETURN 'done' };",
            |_ctx| {},
        );
        assert_eq!(kind, Kind::Either(vec![Kind::Int, Kind::String]));
    }

    #[test]
    fn an_any_exit_absorbs_the_whole_union() {
        // `record::id(...)` (and a recursive UDF call) is `Any` — the top type.
        // A union of a concrete kind with `Any` IS `Any`, not `string | any`,
        // which keeps the mutation checker's `Any` short-circuit working (a
        // workshop-oracle regression: `string | any` tripped a false E2001).
        let kind = block_exit_kind(
            "RETURN { IF $c THEN RETURN 'x' END; RETURN record::id($r) };",
            |_ctx| {},
        );
        assert_eq!(kind, Kind::Any);
    }

    #[test]
    fn evaluates_to_the_final_statement_value_with_let_flow() {
        let parsed = parse_source(
            SourceId::new("flow:test"),
            "RETURN { LET $x = 1; RETURN $x + 1; };",
        )
        .expect("query parses");
        let ast::Statement::Block(block) =
            surrealql_analyzer_syntax::lower::lower_first_statement(&parsed, "Block")
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
