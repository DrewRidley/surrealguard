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
use crate::analyzer::contract::{Contract, Position};
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
    use crate::analyzer::const_eval::BranchReach;

    // Decide each guard's reachability against the *narrowed* env in force at
    // this `IF` — not just literal constants. A guard provably `false` given
    // what narrowing already knows about its subject (`$x` shown non-none by an
    // earlier guard's fall-through, a record pinned to one table, …) makes its
    // branch dead exactly as a constant-false guard does; the first provably-
    // true guard makes every later branch and the `ELSE` dead. Only reachable
    // arms contribute to the value/exit union; dead arms are greyed (4024) and
    // dropped. The env is read before any branch narrowing is applied below, so
    // it reflects the facts that hold on entry to the whole `IF`.
    let reach = crate::analyzer::flow::narrow::branch_reachability_in_env(stmt, ctx.env());

    let mut returns: Vec<Kind> = Vec::new();
    // Pass-through values: the trailing value of each reachable branch through
    // which control can continue past the `IF`.
    let mut pass_through: Vec<Kind> = Vec::new();
    // Whether *every* reachable arm diverges.
    let mut all_arms_diverge = true;

    for (branch, branch_reach) in stmt.branches.iter().zip(&reach.branches) {
        // Conditions are analyzed for their facts (params, dependencies),
        // not their value — but a condition whose kind can never be a bool
        // is worth a warning (truthiness makes it run regardless).
        //
        // A condition is a truthiness test, and truthiness of a collection is
        // its emptiness — engine-verified on 3.0.5: `IF []` takes the ELSE,
        // `IF [{count: 1}]` takes the THEN. So a `SELECT count()` read here is
        // read for its cardinality, and `GROUP ALL` (whose empty result is
        // `[{count: 0}]`, i.e. truthy) would invert the test rather than fix it.
        let condition_fact = ctx.with_cardinality_position(true, |ctx| {
            crate::analyzer::expression::expr_fact(ctx, &branch.condition)
        });
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
        if Contract::condition(Position::IfCond)
            .decide(&condition_kind)
            .is_violation()
        {
            let span = surrealguard_syntax::span::SourceSpan::new(
                ctx.source().clone(),
                branch.condition.span,
            );
            ctx.emit(
                surrealguard_diagnostics::catalog::finding(
                    span,
                    2005,
                    format!(
                        "this IF condition is a `{}`, not a `bool`",
                        crate::render::render_offending(&condition_kind, Some(&Kind::Bool))
                    ),
                )
                .with_help("an IF chooses a branch on a true/false test; the condition must be a bool"),
            );
        }

        match branch_reach {
            BranchReach::Reachable => {
                // The THEN body runs only when the branch condition holds, so it
                // sees what that condition proves. The refinement holds over the
                // body and nothing else — the condition itself still reads the
                // declared kind — so the region is recorded with it and that is
                // what an editor hovers.
                let region = block_span(&branch.body);
                let flow = ctx.with_child_env(|ctx| {
                    crate::analyzer::flow::narrow::narrow_where_true(
                        ctx,
                        &branch.condition.node,
                        region,
                    );
                    analyze_block_flow(ctx, &branch.body)
                });
                returns.extend(flow.returns);
                if !flow.diverges {
                    all_arms_diverge = false;
                    pass_through.push(flow.value);
                }
            }
            // A dead branch never runs: grey its body and drop it from the
            // union. Its body is not analyzed — dead code raises no findings of
            // its own.
            BranchReach::DeadFalse => grey_dead_branch(
                ctx,
                &branch.body,
                "this branch is never taken — its condition is always false",
            ),
            BranchReach::DeadAfterTrue => grey_dead_branch(
                ctx,
                &branch.body,
                "this branch is unreachable — an earlier condition is always true",
            ),
        }
    }

    if reach.else_dead {
        // An always-true branch is guaranteed to run, so the `ELSE` (or the
        // implicit `NONE` fall-through) can never be reached: grey a written
        // `ELSE`, and contribute no fall-through value.
        if let Some(else_branch) = &stmt.else_branch {
            grey_dead_branch(
                ctx,
                else_branch,
                "this ELSE is unreachable — an earlier condition is always true",
            );
        }
    } else if let Some(else_branch) = &stmt.else_branch {
        // The ELSE runs only when every preceding branch condition was false,
        // so it sees the negation of each — one conjunction, not a list.
        let region = block_span(else_branch);
        let flow = ctx.with_child_env(|ctx| {
            crate::analyzer::flow::narrow::narrow_where_all_false(
                ctx,
                stmt.branches.iter().map(|branch| &branch.condition.node),
                region,
            );
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

    // The `IF` diverges only when control cannot fall through: every reachable
    // arm diverges *and* there is a guaranteed terminal arm — either a present
    // `ELSE` (with at least one branch), or an always-true branch that makes
    // the `ELSE`/fall-through dead.
    let has_terminal_arm =
        reach.else_dead || (stmt.else_branch.is_some() && !stmt.branches.is_empty());
    let diverges = has_terminal_arm && all_arms_diverge;

    Flow {
        returns,
        value: Kind::either(pass_through),
        diverges,
    }
}

/// Emits the greyed dead-branch finding (4024) over a provably-dead branch or
/// `ELSE` body. An empty body has nothing to grey, so it is skipped.
fn grey_dead_branch(ctx: &mut AnalysisContext<'_>, body: &ast::Block, message: &str) {
    let Some(range) = block_span(body) else {
        return;
    };
    let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), range);
    ctx.emit(
        surrealguard_diagnostics::catalog::finding(span, 4024, message.to_string())
            .with_tag(surrealguard_diagnostics::FindingTag::Unnecessary),
    );
}

/// The byte range spanning a block's statements (first start .. last end), or
/// `None` for an empty block.
fn block_span(block: &ast::Block) -> Option<surrealguard_syntax::span::ByteRange> {
    let first = block.statements.first()?;
    let last = block.statements.last()?;
    surrealguard_syntax::span::ByteRange::new(first.span.start(), last.span.end()).ok()
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
            // A dynamic guard keeps both branches reachable (a constant guard
            // would fold to a single branch); this exercises the value union.
            "IF $c { LET $v = 1; RETURN $v; } ELSE { RETURN 's'; };",
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

    /// The diagnostics emitted for the first `IfElseStatement` in `source`.
    fn if_else_findings(source: &str) -> Vec<Finding> {
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
        analyze_if_else(&mut ctx, &stmt);
        diagnostics
    }

    #[test]
    fn a_const_true_guard_folds_to_only_the_then_branch() {
        // `IF 1 == 1 { 1 } ELSE { 'x' }` — the ELSE is dead, so the value is
        // just the THEN's `int`, not `int | string`.
        assert_eq!(if_else_kind("IF 1 == 1 { 1 } ELSE { 'x' };"), Kind::Int);
    }

    #[test]
    fn a_const_false_guard_folds_to_only_the_else_branch() {
        // `IF false { 1 } ELSE { 'x' }` — the THEN is dead, so the value is the
        // ELSE's `string`.
        assert_eq!(if_else_kind("IF false { 1 } ELSE { 'x' };"), Kind::String);
    }

    #[test]
    fn an_unknown_guard_still_unions_both_branches() {
        // Regression guard: a non-constant guard folds nothing, so both
        // branches contribute — `int | string`.
        assert_eq!(
            if_else_kind("IF $x { 1 } ELSE { 'x' };"),
            Kind::Either(vec![Kind::Int, Kind::String])
        );
    }

    #[test]
    fn a_const_false_branch_is_greyed_as_dead_code() {
        // The provably-false branch body gets the 4024 finding, tagged
        // Unnecessary so the LSP greys it.
        let findings = if_else_findings("IF 2 > 3 { RETURN 1 } ELSE { RETURN 2 };");
        let dead: Vec<_> = findings
            .iter()
            .filter(|finding| finding.code().number() == 4024)
            .collect();
        assert_eq!(dead.len(), 1, "exactly one dead-branch finding");
        assert_eq!(dead[0].tags(), &[surrealguard_diagnostics::FindingTag::Unnecessary]);
    }

    #[test]
    fn an_unknown_guard_greys_nothing() {
        // Regression guard: a dynamic guard proves nothing, so no branch is
        // dead and no 4024 fires.
        let findings = if_else_findings("IF $x { RETURN 1 } ELSE { RETURN 2 };");
        assert!(findings
            .iter()
            .all(|finding| finding.code().number() != 4024));
    }

    // ---- IF-as-a-value branch checking (routed through `analyze_if_else_flow`
    //      from `statement_value_kind`) ----

    /// The rendered codes a whole query produces (workspace end-to-end).
    fn value_if_codes(query: &str) -> Vec<String> {
        use crate::analysis::{analyze_query, Workspace};
        let mut workspace = Workspace::default();
        analyze_query(&mut workspace, query)
            .diagnostics
            .iter()
            .map(|finding| finding.code().to_string())
            .collect()
    }

    /// How many times `code` fires for `query`.
    fn value_if_count(query: &str, code: &str) -> usize {
        value_if_codes(query)
            .iter()
            .filter(|rendered| rendered.as_str() == code)
            .count()
    }

    #[test]
    fn reachable_if_value_branch_reports_a_check_side_type_error_exactly_once() {
        // The core gap: `'a' + true` in a value-position IF branch is a
        // check-side invariant (2004) that pure inference never triggered.
        // Now it fires — and exactly once, despite re-inference of the
        // subquery kind (dedup via `ctx.emit`).
        let query = "RETURN IF $c { 'a' + true } ELSE { 1 };";
        assert_eq!(value_if_count(query, "E2004"), 1, "codes: {:?}", value_if_codes(query));
    }

    #[test]
    fn reachable_if_value_branch_reports_a_field_error_exactly_once() {
        // A bad field on a schemaful table inside a reachable branch: exactly
        // one E1002, end-to-end (single-emit).
        let query = concat!(
            "DEFINE TABLE thing SCHEMAFULL;\n",
            "DEFINE FIELD name ON thing TYPE string;\n",
            "RETURN IF $c { SELECT badfield FROM thing } ELSE { 1 };",
        );
        assert_eq!(value_if_count(query, "E1002"), 1, "codes: {:?}", value_if_codes(query));
    }

    #[test]
    fn reachable_if_value_branch_reports_a_non_field_rule_exactly_once() {
        // A NON-field rule must fire inside the branch too: an unknown-table
        // SELECT (1001) in a reachable branch, exactly once.
        let query = "RETURN IF $c { SELECT * FROM nonexistent } ELSE { 1 };";
        assert_eq!(value_if_count(query, "E1001"), 1, "codes: {:?}", value_if_codes(query));

        // And a function argument-kind mismatch (5002) inside a branch.
        let arg = "RETURN IF $c { string::len(1) } ELSE { 1 };";
        assert_eq!(value_if_count(arg, "E5002"), 1, "codes: {:?}", value_if_codes(arg));
    }

    #[test]
    fn nested_if_value_in_a_binary_still_emits_a_branch_error_once() {
        // The subquery kind is re-read by the operator check, re-inferring the
        // branch — `ctx.emit` dedup keeps it a single finding.
        let query = "RETURN (IF $c { 'a' + true } ELSE { 2 }) ?? 0;";
        assert_eq!(value_if_count(query, "E2004"), 1, "codes: {:?}", value_if_codes(query));
    }

    #[test]
    fn dead_if_value_branch_body_is_not_checked() {
        // A provably-dead branch (`IF false`) is greyed, never checked: the bad
        // field in its body raises no field error. (The dead branch is still
        // greyed with 4024, matching statement-IF behavior — that is not a
        // check of its contents.)
        let query = concat!(
            "DEFINE TABLE thing SCHEMAFULL;\n",
            "DEFINE FIELD name ON thing TYPE string;\n",
            "RETURN IF false { SELECT badfield FROM thing } ELSE { 1 };",
        );
        assert_eq!(value_if_count(query, "E1002"), 0, "codes: {:?}", value_if_codes(query));

        // Likewise a check-side error in a dead branch stays silent.
        let binary = "RETURN IF false { 'a' + true } ELSE { 1 };";
        assert_eq!(value_if_count(binary, "E2004"), 0, "codes: {:?}", value_if_codes(binary));
    }

    #[test]
    fn if_value_result_kind_is_unchanged_by_the_checking_route() {
        use crate::analysis::{analyze_query, Workspace};
        let response = |query: &str| {
            let mut workspace = Workspace::default();
            analyze_query(&mut workspace, query).response_kind
        };
        // A const-true guard still folds to just the THEN's kind.
        assert_eq!(response("RETURN IF 1 == 1 { 1 } ELSE { 'x' };"), Some(Kind::Int));
        // A dynamic guard still unions both branches.
        assert_eq!(
            response("RETURN IF $c { 1 } ELSE { 'x' };"),
            Some(Kind::Either(vec![Kind::Int, Kind::String]))
        );
    }

    #[test]
    fn if_value_branch_narrowing_does_not_leak_into_the_surrounding_scope() {
        // Checking `IF $x != NONE { ... }` narrows `$x` to its non-none kind
        // *inside* the branch only — an IF-expression has no fall-through, so
        // the outer `$x` binding must remain `option<int>` afterward.
        let parsed = parse_source(
            SourceId::new("flow:test"),
            "LET $y = IF $x != NONE { 1 } ELSE { 2 };",
        )
        .expect("query parses");
        let ast::Statement::Let(stmt) =
            surrealguard_syntax::lower::lower_first_statement(&parsed, "LetStatement")
                .expect("no LetStatement node in tree")
                .node
        else {
            panic!("expected let statement");
        };
        let schema = SchemaIndex::default();
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(
            &schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
        );
        let option_int = Kind::Either(vec![Kind::None, Kind::Int]);
        let span = surrealguard_syntax::span::SourceSpan::new(
            ctx.source().clone(),
            surrealguard_syntax::span::ByteRange::new(0, 1).unwrap(),
        );
        ctx.define_local(
            "x".into(),
            crate::expression::ExpressionFact::new(
                span,
                crate::expression::ExpressionValueClass::Variable,
            )
            .with_kind(option_int.clone()),
        );

        crate::analyzer::flow::let_stmt::analyze_let(&mut ctx, &stmt);

        assert_eq!(
            ctx.env().let_fact("x").and_then(|fact| fact.kind.clone()),
            Some(option_int),
            "the branch guard narrowing must not leak past the IF-expression",
        );
        assert!(
            ctx.env().narrowed_path("x").is_none(),
            "no per-path narrowing should survive the IF-expression",
        );
    }

    /// An `ELSE` is reached only when **every** branch condition failed, so the
    /// negations are a conjunction and must be met, not listed.
    ///
    /// The list could not do it for a field path: each effect resolved the
    /// path's kind through the schema from the *declared* kind, so the second
    /// negation overwrote the first instead of composing with it, and the
    /// `ELSE` saw a `none` the first branch had already ruled out.
    #[test]
    fn the_else_meets_every_branch_negation_on_one_field_path() {
        use crate::analyzer::flow::narrow::with_fact_layer;
        use surrealdb_types::KindLiteral;

        let subject = Kind::Literal(KindLiteral::Object(std::collections::BTreeMap::from([(
            "a".to_string(),
            Kind::either(vec![Kind::None, Kind::Null, Kind::String]),
        )])));
        let source = "IF $x.a = NONE { RETURN 'n' } \
                      ELSE IF $x.a = NULL { RETURN 'l' } \
                      ELSE { RETURN $x.a };";
        let else_value = |kind: Kind| match kind {
            Kind::Either(variants) => variants
                .into_iter()
                .filter(|variant| {
                    !matches!(variant, Kind::Literal(KindLiteral::String(text))
                        if text == "n" || text == "l")
                })
                .collect::<Vec<_>>(),
            other => vec![other],
        };

        // Both sentinels were ruled out by the two failed branches, and only
        // one of them survived the overwrite.
        assert_eq!(
            else_value(with_fact_layer(false, || bound_if(subject.clone(), false, source).0)),
            vec![Kind::String, Kind::None]
        );
        assert_eq!(
            else_value(with_fact_layer(true, || bound_if(subject, false, source).0)),
            vec![Kind::String]
        );
    }

    #[test]
    fn a_const_true_guard_greys_the_dead_else() {
        // The always-true THEN makes the ELSE unreachable — the ELSE body is
        // greyed.
        let findings = if_else_findings("IF true { RETURN 1 } ELSE { RETURN 2 };");
        assert_eq!(
            findings
                .iter()
                .filter(|finding| finding.code().number() == 4024)
                .count(),
            1
        );
    }

    // ---- Narrowing-aware dead-branch folding ------------------------------

    /// Binds `$x` to `kind`, analyzes the first `IF`, and returns `(kind,
    /// findings)`.
    /// Binds `$x` to `kind`, analyzes the first `IF`, and returns `(kind,
    /// findings)`. When `narrowed`, `$x` is marked as flow-narrowed (the state
    /// a prior guard leaves); otherwise it is a plain base binding — the
    /// distinction the dead-branch verdict gates on.
    fn bound_if(kind: Kind, narrowed: bool, source: &str) -> (Kind, Vec<Finding>) {
        use crate::expression::{ExpressionFact, ExpressionValueClass};
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
        let span = surrealguard_syntax::span::SourceSpan::new(
            ctx.source().clone(),
            surrealguard_syntax::span::ByteRange::new(0, 1).unwrap(),
        );
        let fact = ExpressionFact::new(span, ExpressionValueClass::Variable).with_kind(kind);
        if narrowed {
            ctx.narrow_local("x".into(), fact);
        } else {
            ctx.define_local("x".into(), fact);
        }
        let result = analyze_if_else(&mut ctx, &stmt);
        (result, diagnostics)
    }

    fn dead_count(findings: &[Finding]) -> usize {
        findings
            .iter()
            .filter(|finding| finding.code().number() == 4024)
            .count()
    }

    fn record(table: &str) -> Kind {
        Kind::Record(vec![surrealdb_types::Table::from(table)])
    }

    #[test]
    fn none_guard_on_a_flow_narrowed_non_none_subject_greys_the_then_branch() {
        // `$x` was narrowed to a bare `record<b>` by a prior guard, so a
        // re-check `IF $x = NONE { ... }` can never run: the THEN is dead and
        // only the ELSE contributes.
        let (kind, findings) =
            bound_if(record("b"), true, "IF $x = NONE { RETURN 1 } ELSE { RETURN 2 };");
        assert_eq!(dead_count(&findings), 1, "exactly one greyed branch");
        assert_eq!(
            findings
                .iter()
                .find(|f| f.code().number() == 4024)
                .unwrap()
                .tags(),
            &[surrealguard_diagnostics::FindingTag::Unnecessary]
        );
        // Only the ELSE runs → the IF's value is the ELSE's `int`.
        assert_eq!(kind, Kind::Int);
    }

    #[test]
    fn not_none_guard_on_a_flow_narrowed_non_none_subject_greys_the_else() {
        // `$x != NONE` is always true for the narrowed record, so the ELSE is dead.
        let (kind, findings) =
            bound_if(record("b"), true, "IF $x != NONE { RETURN 1 } ELSE { RETURN 2 };");
        assert_eq!(dead_count(&findings), 1, "the ELSE is greyed");
        // Only the THEN runs → the value is the THEN's `int`.
        assert_eq!(kind, Kind::Int);
    }

    #[test]
    fn table_discriminant_that_cannot_hold_greys_the_then_branch() {
        // `$x` narrowed to `record<b>`; `type::table($x) = 'a'` can never hold.
        let (_kind, findings) = bound_if(
            record("b"),
            true,
            "IF type::table($x) = 'a' { RETURN 1 } ELSE { RETURN 2 };",
        );
        assert_eq!(dead_count(&findings), 1, "the dead discriminant THEN is greyed");
    }

    #[test]
    fn a_declared_non_optional_param_defensive_check_greys_nothing() {
        // The refinement's headline regression: a bare declared `record<b>`
        // param (never flow-narrowed) with an idiomatic defensive
        // `IF $x = NONE { ... }` is technically dead but must NOT be greyed —
        // it is a defensive check, not conditional-flow dead code.
        let (_kind, findings) =
            bound_if(record("b"), false, "IF $x = NONE { RETURN 1 } ELSE { RETURN 2 };");
        assert_eq!(dead_count(&findings), 0, "defensive check on a base binding is kept");

        // Likewise the discriminant and is_record forms on a base binding.
        let (_k, f) = bound_if(
            record("b"),
            false,
            "IF type::table($x) = 'a' { RETURN 1 } ELSE { RETURN 2 };",
        );
        assert_eq!(dead_count(&f), 0);
    }

    #[test]
    fn optional_subject_first_check_greys_nothing() {
        // A `$x : option<record>` still might be none, so even once narrowing is
        // active the first `IF $x = NONE` proves nothing — no branch is dead.
        let option = Kind::Either(vec![Kind::None, Kind::Record(vec![])]);
        let (kind, findings) =
            bound_if(option, true, "IF $x = NONE { RETURN 1 } ELSE { RETURN 2 };");
        assert_eq!(dead_count(&findings), 0, "an unknown subject greys nothing");
        // Both branches live → `int` union collapses to `int`.
        assert_eq!(kind, Kind::Int);
    }

    /// The number of `code` findings for `source` analyzed as a workspace.
    fn workspace_code_count(source: &str, code: u16) -> usize {
        let mut workspace = crate::analysis::Workspace::default();
        workspace.add_virtual_source("if-dead-e2e".into(), source.into());
        crate::analysis::analyze_workspace(&workspace)
            .diagnostics
            .iter()
            .filter(|finding| finding.code().number() == code)
            .count()
    }

    #[test]
    fn redundant_recheck_after_a_diverging_none_guard_is_dead_and_unchecked() {
        // The user's case: the first guard's fall-through narrows `$x` to
        // non-none, so the *second* `IF $x = NONE` is provably dead — greyed
        // once, and its body (a bad SELECT) is never analyzed.
        let source = "DEFINE TABLE b SCHEMAFULL;\n\
             DEFINE FUNCTION fn::f($x: option<record<b>>) {\n\
                IF $x = NONE THEN RETURN false END;\n\
                IF $x = NONE { SELECT foo FROM nonexistent };\n\
                RETURN 3;\n\
             };";
        // Exactly one dead-branch finding (the second, redundant guard).
        assert_eq!(workspace_code_count(source, 4024), 1);
        // Its body is never analyzed → the unknown-table SELECT raises no 1001.
        assert_eq!(workspace_code_count(source, 1001), 0, "dead body not checked");
    }

    #[test]
    fn nested_recheck_inside_a_narrowed_branch_is_dead() {
        // `IF $x != NONE { ...; IF $x = NONE { <dead> } }` — inside the THEN,
        // `$x` is narrowed non-none, so the inner re-check is provably dead.
        let source = "DEFINE TABLE b SCHEMAFULL;\n\
             DEFINE FUNCTION fn::f($x: option<record<b>>) {\n\
                IF $x != NONE {\n\
                   IF $x = NONE { SELECT foo FROM nonexistent };\n\
                };\n\
                RETURN 3;\n\
             };";
        assert_eq!(workspace_code_count(source, 4024), 1, "inner re-check greyed");
        assert_eq!(workspace_code_count(source, 1001), 0, "dead body not checked");
    }
}
