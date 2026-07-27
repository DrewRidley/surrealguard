//! Control-flow type narrowing: where the flow engine meets the fact layer.
//!
//! A guard condition partitions a value space; each reachable region
//! downstream sees the places the guard constrained narrowed to the kinds
//! consistent with it. *Which* facts a condition proves is not decided here —
//! that is [`crate::analyzer::facts`], which lowers any condition to a guard IR
//! and interprets it as a refinement of the `Kind` lattice. This module is the
//! other half: it asks that layer what a region knows, and applies the answer
//! to the flow environment so every downstream read (function-argument checks,
//! field access, hover) sees the narrowed kind.
//!
//! The three regions a guard creates each read the same interpretation, so a
//! `THEN` body, an `ELSE` body and the fall-through past a diverging guard can
//! never disagree about what the same condition proves.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;
use surrealguard_syntax::span::ByteRange;

use crate::analyzer::const_eval::{BranchReach, Reachability};
use crate::analyzer::context::AnalysisContext;
use crate::analyzer::facts::{
    eval, guard_of, Bindings, DiscriminantKind, Facts, KindOracle, Place, PlaceRoot, Term, Verdict,
};
use crate::statement_env::StatementEnv;

/// The refinements `cond` proves in the region where it evaluates to
/// `polarity`, read straight from the [expression-fact
/// layer](crate::analyzer::facts).
pub(crate) fn guard_facts(cond: &ast::Expr, polarity: bool, env: &StatementEnv) -> Facts {
    guard_of(cond, polarity, Some(env)).facts(&EnvOracle(env))
}

/// The refinements proved in the region reached when **every** one of these
/// conditions was false: an `ELSE`, or the fall-through past an `IF` whose
/// every arm diverged.
///
/// The conjunction is explicit — one `Guard::All` of the negations, interpreted
/// once — rather than a concatenation of per-branch effect lists applied in
/// order. That matters where two branches claim about the same place: the
/// interpreter composes the claims, where the list applied the second to the
/// *declared* kind and overwrote the first.
pub(crate) fn all_false_facts<'a>(
    conds: impl IntoIterator<Item = &'a ast::Expr>,
    env: &StatementEnv,
) -> Facts {
    let negations = conds
        .into_iter()
        .map(|cond| guard_of(cond, false, Some(env)))
        .collect();
    crate::analyzer::facts::Guard::All(negations).facts(&EnvOracle(env))
}

/// The kinds the flow environment holds, as the fact layer reads them.
///
/// A bare param resolves to its binding; a field path resolves only when an
/// earlier narrowing recorded one. The *declared* kind of a field path is not
/// available here — stepping it needs the schema, which the guard callers do
/// not pass — so a place it cannot answer is answered `None`, and the atoms
/// that need a kind (membership) simply prove nothing.
struct EnvOracle<'a>(&'a StatementEnv);

impl KindOracle for EnvOracle<'_> {
    fn kind_of(&self, place: &Place) -> Option<Kind> {
        let PlaceRoot::Param(name) = &place.root else {
            return None;
        };
        if place.path.is_empty() {
            return self.0.let_fact(name)?.kind.clone();
        }
        self.0.narrowed_path(&place.key()?).cloned()
    }

    /// A bare param counts as narrowed when its binding was *rebound* by a
    /// guard rather than merely declared; a field path counts when a narrowing
    /// override was recorded for it. That is exactly the pair `path_kind` gates
    /// on today, and it is what keeps a defensive `IF $p = NONE` on a declared
    /// non-optional `$p` from being greyed.
    fn is_flow_narrowed(&self, place: &Place) -> bool {
        let PlaceRoot::Param(name) = &place.root else {
            return false;
        };
        if place.path.is_empty() {
            return self.0.is_param_narrowed(name);
        }
        place
            .key()
            .is_some_and(|key| self.0.narrowed_path(&key).is_some())
    }
}

/// The param name, when `expr` is a bare `type::table($param)` call — used to
/// seed the indirect-discriminant map from `LET $t = type::table($param)`.
/// (Only bare params are recorded as indirect discriminants.)
pub(crate) fn type_table_arg(expr: &ast::Expr) -> Option<String> {
    let Term::Discriminant {
        of,
        kind: DiscriminantKind::RecordTable,
    } = eval(expr, Bindings::NONE)
    else {
        return None;
    };
    let PlaceRoot::Param(param) = of.root else {
        return None;
    };
    of.path.is_empty().then_some(param)
}

/// Narrows the current scope by what `cond` proves in the region where it
/// holds — an `IF`'s THEN body — recording `region` as the span the refinement
/// covers so an editor can read it back.
///
/// Called *inside* the child scope the body is analyzed in, so the narrowing
/// dies with the body.
pub(crate) fn narrow_where_true(
    ctx: &mut AnalysisContext<'_>,
    cond: &ast::Expr,
    region: Option<ByteRange>,
) {
    let facts = guard_facts(cond, true, ctx.env());
    apply_facts_over(ctx, &facts, region);
}

/// Narrows the current scope by the negation of **every** one of `conds`: an
/// `ELSE`, or the fall-through past an `IF` whose every arm diverged. The two
/// are the same region reached two ways, so they read the same function and
/// cannot narrow differently.
pub(crate) fn narrow_where_all_false<'a>(
    ctx: &mut AnalysisContext<'_>,
    conds: impl IntoIterator<Item = &'a ast::Expr>,
    region: Option<ByteRange>,
) {
    let facts = all_false_facts(conds, ctx.env());
    apply_facts_over(ctx, &facts, region);
}

/// Applies [`Facts`] to the current scope, recording no region.
pub(crate) fn apply_facts(ctx: &mut AnalysisContext<'_>, facts: &Facts) {
    apply_facts_over(ctx, facts, None);
}

/// [`apply_facts`], additionally recording the source region each refinement
/// holds over.
///
/// The environment alone cannot answer an editor's question — it is a moving
/// point-in-time value, and by the time hover runs, analysis is over. So the
/// callers that *know* the region a refinement covers (a branch body, the
/// statements after a diverging guard) pass it here, and it is recorded
/// alongside the narrowed kind. Callers that don't — the `AND`/`OR`
/// short-circuit inside a single expression — use [`apply_facts`] and record
/// nothing, which leaves the editor showing the declared kind exactly as
/// before.
///
/// Place by place: a bare param rebinds its binding, a field path records a per-path override keyed on
/// the exact `param.field…` string, and a refinement that does not tighten the
/// kind currently in force records nothing.
///
/// Row-rooted places are skipped. A `Place` can name one, but the flow
/// environment binds params — the row side is the projection's business
/// (`data::select::narrow_row_by_facts`).
///
/// Refinements are applied in place order, and each reads the environment as
/// the previous one left it, so a bare param is narrowed before the paths that
/// read through it.
pub(crate) fn apply_facts_over(
    ctx: &mut AnalysisContext<'_>,
    facts: &Facts,
    region: Option<ByteRange>,
) {
    for (place, refinement) in facts.iter() {
        let PlaceRoot::Param(param) = &place.root else {
            continue;
        };
        // A subscript ends a path: only the exact written field path is
        // refinable, so a place carrying one names nothing this can key.
        let Some(fields) = place.field_path() else {
            continue;
        };
        let Some(base) = ctx.env().let_fact(param).cloned() else {
            continue;
        };
        let Some(base_kind) = base.kind.clone() else {
            continue;
        };
        if fields.is_empty() {
            let Some(narrowed) = refinement.apply(&base_kind) else {
                continue;
            };
            let mut new_fact = base;
            new_fact.kind = Some(narrowed.clone());
            // Mark this as a flow narrowing (not a base binding) so dead-branch
            // folding may draw a verdict from the tightened kind.
            ctx.narrow_local(param.clone(), new_fact);
            if let Some(region) = region {
                ctx.record_narrowing(param.clone(), region, narrowed, facts.proof(place));
            }
        } else {
            // Resolve the path's declared kind through the schema, then narrow
            // and record it under the exact path key.
            let Some(current) = crate::analyzer::expression::infer::step_field_path(
                &base_kind,
                &fields,
                ctx.schema(),
            ) else {
                continue;
            };
            let Some(narrowed) = refinement.apply(&current) else {
                continue;
            };
            let key = format!("{param}.{}", fields.join("."));
            if let Some(region) = region {
                ctx.record_narrowing(key.clone(), region, narrowed.clone(), facts.proof(place));
            }
            ctx.define_narrowed_path(key, narrowed);
        }
    }
}

/// Env-aware branch reachability: the narrowing analogue of
/// [`crate::analyzer::const_eval::branch_reachability`]. It walks the branches
/// in order, drawing each guard's [`Verdict`] against `env`, and maps it to the
/// same [`BranchReach`] lattice the constant path uses:
///
/// * `AlwaysFalse` → [`BranchReach::DeadFalse`];
/// * the first `AlwaysTrue` → [`BranchReach::Reachable`] and every later
///   branch + the `ELSE` are dead;
/// * `Unknown` → [`BranchReach::Reachable`], killing nothing.
///
/// **Why the base `env` is sound for every branch.** The env actually in force
/// at branch *N* is `env` narrowed by the negation of branches `0..N` (each was
/// false to reach *N*) — a *subset* of the values `env` allows. A guard proven
/// `AlwaysFalse` over the whole of `env` is `AlwaysFalse` over any subset, and
/// likewise for `AlwaysTrue`; so verdicts drawn against the un-accumulated base
/// `env` can only *under*-report dead branches, never grey a live one. (That is
/// the deliberately conservative trade: soundness over completeness.)
pub(crate) fn branch_reachability_in_env(
    stmt: &ast::IfElseStmt,
    env: &StatementEnv,
) -> Reachability {
    let mut branches = Vec::with_capacity(stmt.branches.len());
    // Set once an earlier branch is proven always-taken.
    let mut taken = false;
    for branch in &stmt.branches {
        if taken {
            branches.push(BranchReach::DeadAfterTrue);
            continue;
        }
        match guard_of(&branch.condition.node, true, Some(env)).verdict(&EnvOracle(env)) {
            Verdict::AlwaysFalse => branches.push(BranchReach::DeadFalse),
            Verdict::AlwaysTrue => {
                branches.push(BranchReach::Reachable);
                taken = true;
            }
            Verdict::Unknown => branches.push(BranchReach::Reachable),
        }
    }
    Reachability {
        branches,
        else_dead: taken,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealdb_types::Table;
    use surrealguard_syntax::ast;
    use surrealguard_syntax::lower::lower_first_expr;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    fn cond(query: &str) -> ast::Expr {
        let parsed = parse_source(SourceId::new("narrow:test"), query).expect("parses");
        lower_first_expr(&parsed, "BinaryExpression")
            .expect("has a binary condition")
            .node
    }

    fn call_cond(query: &str) -> ast::Expr {
        let parsed = parse_source(SourceId::new("narrow:test"), query).expect("parses");
        lower_first_expr(&parsed, "FunctionCall")
            .expect("has a call condition")
            .node
    }

    // --- End-to-end narrowing over the analysis pipeline -------------------

    /// The number of findings with `code` when the source is analyzed as a
    /// workspace (schema + function bodies).
    fn code_count(source: &str, code: u16) -> usize {
        let mut workspace = crate::analysis::Workspace::default();
        workspace.add_virtual_source("narrow-e2e".into(), source.into());
        let output = crate::analysis::analyze_workspace(&workspace);
        output
            .diagnostics
            .iter()
            .filter(|finding| finding.code().number() == code)
            .count()
    }

    const TABLES: &str = "DEFINE TABLE file SCHEMAFULL;\n\
         DEFINE TABLE folder SCHEMAFULL;\n\
         DEFINE TABLE organization SCHEMAFULL;\n\
         DEFINE FUNCTION fn::file_only($f: record<file>) { RETURN true; };\n\
         DEFINE FUNCTION fn::folder_only($f: record<folder>) { RETURN true; };\n\
         DEFINE FUNCTION fn::org_only($o: record<organization>) { RETURN true; };\n";

    #[test]
    fn none_early_return_narrows_the_optional_param() {
        // An early-return NONE guard clears the 5002 an unguarded
        // `option<record<organization>>` would raise at the file-only call.
        let guarded = format!(
            "{TABLES}\
             DEFINE FUNCTION fn::caller($o: option<record<organization>>) {{\n\
                IF $o = NONE THEN RETURN false END;\n\
                RETURN fn::org_only($o);\n\
             }};"
        );
        assert_eq!(code_count(&guarded, 5002), 0, "guarded should be clean");

        // The same call WITHOUT the guard genuinely fails — proving the guard
        // is what clears it.
        let unguarded = format!(
            "{TABLES}\
             DEFINE FUNCTION fn::caller($o: option<record<organization>>) {{\n\
                RETURN fn::org_only($o);\n\
             }};"
        );
        assert_eq!(code_count(&unguarded, 5002), 1, "unguarded must fire");
    }

    #[test]
    fn type_table_discriminant_narrows_the_fall_through_and_then_branch() {
        // Fall-through: after the diverging folder guard, `$r` is `record<file>`.
        let fall_through = format!(
            "{TABLES}\
             DEFINE FUNCTION fn::rec($r: record<file | folder>) {{\n\
                IF type::table($r) = 'folder' THEN RETURN fn::folder_only($r) END;\n\
                RETURN fn::file_only($r);\n\
             }};"
        );
        assert_eq!(code_count(&fall_through, 5002), 0, "narrowed both sides");
    }

    #[test]
    fn branch_merge_widens_back_to_the_union_at_the_join() {
        // Neither branch diverges, so past the join `$r` is the full union
        // again — the file-only call must still fire.
        let widened = format!(
            "{TABLES}\
             DEFINE FUNCTION fn::rec($r: record<file | folder>) {{\n\
                IF type::table($r) = 'folder' {{ LET $a = 1; }} ELSE {{ LET $b = 2; }};\n\
                RETURN fn::file_only($r);\n\
             }};"
        );
        assert_eq!(code_count(&widened, 5002), 1, "union not leaked-narrowed");
    }

    #[test]
    fn indirect_discriminant_narrows_through_a_let_binding() {
        // `LET $t = type::table($r); IF $t = 'folder' THEN ...` narrows `$r`.
        let indirect = format!(
            "{TABLES}\
             DEFINE FUNCTION fn::rec($r: record<file | folder>) {{\n\
                LET $t = type::table($r);\n\
                IF $t = 'folder' THEN RETURN fn::folder_only($r) END;\n\
                RETURN fn::file_only($r);\n\
             }};"
        );
        assert_eq!(code_count(&indirect, 5002), 0);
    }

    // --- Field-path narrowing (`$file.folder != NONE`) ---------------------

    /// `file` has an optional `folder` link and a sibling optional link; the
    /// callee wants the non-optional `record<folder>`.
    const FIELD_PATH_TABLES: &str = "DEFINE TABLE folder SCHEMAFULL;\n\
         DEFINE TABLE file SCHEMAFULL;\n\
         DEFINE FIELD folder ON file TYPE option<record<folder>>;\n\
         DEFINE FIELD sibling ON file TYPE option<record<folder>>;\n\
         DEFINE FUNCTION fn::folder_only($f: record<folder>) { RETURN true; };\n";

    #[test]
    fn field_path_none_guard_narrows_the_and_right_operand() {
        // `$file.folder != NONE AND fn::folder_only($file.folder)` — the right
        // conjunct only runs when the path is non-none, so it type-checks.
        let guarded = format!(
            "{FIELD_PATH_TABLES}\
             DEFINE FUNCTION fn::caller($file: record<file>) {{\n\
                IF $file.folder != NONE AND fn::folder_only($file.folder) THEN RETURN true END;\n\
                RETURN false;\n\
             }};"
        );
        assert_eq!(
            code_count(&guarded, 5002),
            0,
            "AND-right sees the narrowed path"
        );

        // The same call WITHOUT the guard genuinely fails against the
        // `option<record<folder>>` argument — proving the guard is what clears it.
        let unguarded = format!(
            "{FIELD_PATH_TABLES}\
             DEFINE FUNCTION fn::caller($file: record<file>) {{\n\
                RETURN fn::folder_only($file.folder);\n\
             }};"
        );
        assert_eq!(code_count(&unguarded, 5002), 1, "unguarded must fire");
    }

    #[test]
    fn field_path_none_guard_narrows_the_then_branch() {
        // `IF $file.folder != NONE THEN ... END` narrows the path in the body.
        let guarded = format!(
            "{FIELD_PATH_TABLES}\
             DEFINE FUNCTION fn::caller($file: record<file>) {{\n\
                IF $file.folder != NONE THEN RETURN fn::folder_only($file.folder) END;\n\
                RETURN false;\n\
             }};"
        );
        assert_eq!(
            code_count(&guarded, 5002),
            0,
            "THEN body sees the narrowed path"
        );
    }

    #[test]
    fn field_path_guard_does_not_narrow_a_sibling_path() {
        // The must-not-narrow boundary: guarding `$file.folder` proves nothing
        // about `$file.sibling`, which is still `option<record<folder>>`.
        let sibling = format!(
            "{FIELD_PATH_TABLES}\
             DEFINE FUNCTION fn::caller($file: record<file>) {{\n\
                IF $file.folder != NONE AND fn::folder_only($file.sibling) THEN RETURN true END;\n\
                RETURN false;\n\
             }};"
        );
        assert_eq!(
            code_count(&sibling, 5002),
            1,
            "the sibling path must not be narrowed"
        );
    }

    // --- `$auth` seed + narrowing (design §4 Phase 1) ----------------------

    /// `$auth` seeds as `option<record>` in every env, so a callee wanting a
    /// concrete/non-optional record needs a guard to clear the mismatch.
    const AUTH_TABLES: &str = "DEFINE TABLE user SCHEMAFULL;\n\
         DEFINE FUNCTION fn::user_only($u: record<user>) { RETURN true; };\n\
         DEFINE FUNCTION fn::any_record($r: record) { RETURN true; };\n";

    #[test]
    fn auth_seeds_as_option_record_so_an_unguarded_record_call_fires() {
        // Baseline: `$auth : option<record>` is not assignable to `record` —
        // proving the seed carries the NONE and reaches a `fn::` body.
        let unguarded = format!(
            "{AUTH_TABLES}\
             DEFINE FUNCTION fn::caller() {{\n\
                RETURN fn::any_record($auth);\n\
             }};"
        );
        assert_eq!(
            code_count(&unguarded, 5002),
            1,
            "option<record> is not record"
        );
    }

    #[test]
    fn auth_none_guard_narrows_to_record_in_the_then_branch() {
        // `IF $auth != NONE THEN ...` strips the NONE (existing narrow path),
        // leaving the open `record`, so the `fn::any_record($auth)` call clears.
        let guarded = format!(
            "{AUTH_TABLES}\
             DEFINE FUNCTION fn::caller() {{\n\
                IF $auth != NONE THEN RETURN fn::any_record($auth) END;\n\
                RETURN false;\n\
             }};"
        );
        assert_eq!(
            code_count(&guarded, 5002),
            0,
            "!= NONE narrows $auth to record"
        );
    }

    #[test]
    fn auth_type_is_record_with_table_narrows_to_that_table() {
        // `type::is_record($auth, 'user')` narrows `$auth` to `record<user>` in
        // the THEN branch, so the `fn::user_only($auth)` call type-checks.
        let guarded = format!(
            "{AUTH_TABLES}\
             DEFINE FUNCTION fn::caller() {{\n\
                IF type::is_record($auth, 'user') THEN RETURN fn::user_only($auth) END;\n\
                RETURN false;\n\
             }};"
        );
        assert_eq!(
            code_count(&guarded, 5002),
            0,
            "positive branch narrows to record<user>"
        );

        // The same call without the guard genuinely fails — the guard is what
        // clears it (also proves the double-colon spelling normalizes).
        let colon = format!(
            "{AUTH_TABLES}\
             DEFINE FUNCTION fn::caller() {{\n\
                IF type::is::record($auth, 'user') THEN RETURN fn::user_only($auth) END;\n\
                RETURN false;\n\
             }};"
        );
        assert_eq!(
            code_count(&colon, 5002),
            0,
            "type::is::record normalizes to type::is_record"
        );
    }

    // --- AND-operand narrowing (occurrence typing over `A AND B`) ----------

    /// Callees demand a non-optional/concrete value; the `AND` guards prove it.
    const AND_TABLES: &str = "DEFINE TABLE user SCHEMAFULL;\n\
         DEFINE FUNCTION fn::takes_record($r: record) { RETURN true; };\n\
         DEFINE FUNCTION fn::takes_int($n: int) { RETURN true; };\n\
         DEFINE FUNCTION fn::two($a: record, $b: record) { RETURN true; };\n";

    #[test]
    fn and_none_guard_narrows_the_sibling_function_argument() {
        // `($x != NONE) AND fn::takes_record($x)` — the right conjunct runs only
        // when `$x` is non-none, so its argument sees `record<user>`.
        let guarded = format!(
            "{AND_TABLES}\
             DEFINE FUNCTION fn::caller($x: option<record<user>>) {{\n\
                IF $x != NONE AND fn::takes_record($x) THEN RETURN true END;\n\
                RETURN false;\n\
             }};"
        );
        assert_eq!(code_count(&guarded, 5002), 0, "AND-right sees non-none $x");

        // Unguarded, the same call genuinely fails against `option<record<user>>`.
        let unguarded = format!(
            "{AND_TABLES}\
             DEFINE FUNCTION fn::caller($x: option<record<user>>) {{\n\
                RETURN fn::takes_record($x);\n\
             }};"
        );
        assert_eq!(code_count(&unguarded, 5002), 1, "unguarded must fire");
    }

    #[test]
    fn and_in_guard_narrows_the_sibling_function_argument() {
        // `($s IN $ints) AND fn::takes_int($s)` — occurrence typing narrows the
        // `option<int>` subject to the collection's `int` element in the arg.
        let guarded = format!(
            "{AND_TABLES}\
             DEFINE FUNCTION fn::caller($s: option<int>, $ints: array<int>) {{\n\
                IF $s IN $ints AND fn::takes_int($s) THEN RETURN true END;\n\
                RETURN false;\n\
             }};"
        );
        assert_eq!(
            code_count(&guarded, 5002),
            0,
            "IN narrows $s to int in the arg"
        );

        // Unguarded, `option<int>` is not assignable to the `int` param.
        let unguarded = format!(
            "{AND_TABLES}\
             DEFINE FUNCTION fn::caller($s: option<int>) {{\n\
                RETURN fn::takes_int($s);\n\
             }};"
        );
        assert_eq!(code_count(&unguarded, 5002), 1, "unguarded must fire");
    }

    #[test]
    fn in_guard_narrows_the_then_branch() {
        // `IF $s IN $ints THEN fn::takes_int($s) END` — the IN effect flows into
        // the branch body the same as any positive guard.
        let guarded = format!(
            "{AND_TABLES}\
             DEFINE FUNCTION fn::caller($s: option<int>, $ints: array<int>) {{\n\
                IF $s IN $ints THEN RETURN fn::takes_int($s) END;\n\
                RETURN false;\n\
             }};"
        );
        assert_eq!(
            code_count(&guarded, 5002),
            0,
            "THEN body sees the narrowed $s"
        );
    }

    #[test]
    fn and_chain_threads_all_prior_positive_effects() {
        // `A AND B AND C`: the last conjunct sees BOTH earlier effects, so
        // `fn::two($x, $y)` type-checks only if the union of effects reached it.
        let guarded = format!(
            "{AND_TABLES}\
             DEFINE FUNCTION fn::caller($x: option<record<user>>, $y: option<record<user>>) {{\n\
                IF $x != NONE AND $y != NONE AND fn::two($x, $y) THEN RETURN true END;\n\
                RETURN false;\n\
             }};"
        );
        assert_eq!(
            code_count(&guarded, 5002),
            0,
            "both $x and $y narrowed for the tail call"
        );

        // Unguarded, both arguments fail — proving the chain cleared two findings.
        let unguarded = format!(
            "{AND_TABLES}\
             DEFINE FUNCTION fn::caller($x: option<record<user>>, $y: option<record<user>>) {{\n\
                RETURN fn::two($x, $y);\n\
             }};"
        );
        assert_eq!(code_count(&unguarded, 5002), 2, "both args fire unguarded");
    }

    #[test]
    fn or_does_not_narrow_the_sibling_operand() {
        // `($x != NONE) OR fn::takes_record($x)` — the right disjunct runs only
        // when `$x` IS none, so it must NOT be narrowed and the call still fires.
        let or_guarded = format!(
            "{AND_TABLES}\
             DEFINE FUNCTION fn::caller($x: option<record<user>>) {{\n\
                IF $x != NONE OR fn::takes_record($x) THEN RETURN true END;\n\
                RETURN false;\n\
             }};"
        );
        assert_eq!(code_count(&or_guarded, 5002), 1, "OR narrows nothing");
    }

    #[test]
    fn and_narrowing_does_not_leak_past_the_and_expression() {
        // The narrowing is scoped to the right operand: a later use of `$x`
        // outside the `AND` is unnarrowed, so the record call still fires.
        let leaky = format!(
            "{AND_TABLES}\
             DEFINE FUNCTION fn::caller($x: option<record<user>>) {{\n\
                LET $ok = $x != NONE AND true;\n\
                RETURN fn::takes_record($x);\n\
             }};"
        );
        assert_eq!(
            code_count(&leaky, 5002),
            1,
            "narrowing must not leak past the AND"
        );
    }

    // --- The design's table, as behaviour ----------------------------------

    /// The rendered kind of the last statement (or last `LET` binding) of
    /// `query`, analyzed against `schema`.
    fn response_kind(schema: &str, query: &str) -> String {
        let mut workspace = crate::analysis::Workspace::default();
        workspace.add_virtual_source("schema".into(), schema.into());
        let source = workspace.add_virtual_source("query".into(), query.into());
        let analysis = crate::analysis::analyze_workspace(&workspace);
        let output = analysis.sources.get(&source).expect("the query source");
        output
            .let_bindings
            .last()
            .and_then(|binding| binding.kind.clone())
            .or_else(|| {
                output
                    .statements
                    .iter()
                    .rev()
                    .find_map(|statement| statement.response_kind.clone())
            })
            .map_or_else(
                || "unknown".to_string(),
                |kind| crate::render::render_kind(&kind),
            )
    }

    /// The cases from the expression-fact design's table that the vendored
    /// corpus does not carry, pinned as behaviour.
    ///
    /// Each was a *hole* in the recognizer set this module replaced: a fact the
    /// analyzer could state in one spelling and not another, or on one side of
    /// a guard and not the other. They lived in a both-ways test comparing the
    /// two paths while both existed; with one path left, what is worth keeping
    /// is the answer, and the note saying what used to come out instead.
    ///
    /// The corpus covers the rest — the three table-discriminant spellings, the
    /// three membership spellings, the De Morgan pairs, `!(x = NONE)`,
    /// `type::is_none`, a bare truthiness guard on a param, and a literal-union
    /// field compared to one of its members.
    #[test]
    fn the_designs_remaining_cases_hold() {
        const SCHEMA: &str = "\
            DEFINE TABLE user SCHEMAFULL;\n\
            DEFINE FIELD name ON user TYPE string;\n\
            DEFINE FIELD email ON user TYPE option<string>;\n\
            DEFINE FIELD age ON user TYPE option<int>;\n\
            DEFINE FIELD status ON user TYPE 'active' | 'inactive' | 'banned';\n\
            DEFINE FIELD note ON user TYPE option<string | null>;\n";

        // (case, query, kind — with what the recognizers used to answer)
        let cases = [
            (
                // F13 — a bare field as a `WHERE` is a truthiness guard. It was
                // not a recognized shape at all, so it narrowed nothing.
                "F13",
                "SELECT email FROM user WHERE email;",
                "array<{ email: string }>",
            ),
            (
                // F15 — a kind predicate over a row field. The predicate was
                // recognized on a param and not on a row.
                "F15",
                "SELECT email FROM user WHERE type::is_string(email);",
                "array<{ email: string }>",
            ),
            (
                // F19 — `!=` against one member of a literal union subtracts
                // it. The row recognizer settled `=` only.
                "F19",
                "SELECT status FROM user WHERE status != 'banned';",
                "array<{ status: 'active' | 'inactive' }>",
            ),
            (
                // F11 — a kind predicate on a param, read through a field
                // access in the guarded branch. `any | string` before.
                "F11",
                "LET $x = (SELECT name FROM ONLY user LIMIT 1);\n\
                 LET $r = IF type::is_object($x) THEN $x.name ELSE 'x' END;",
                "string",
            ),
            (
                // The fall-through past a multi-branch diverging guard is
                // reached only when EVERY condition failed, so the negations
                // are a conjunction. Applied as a list — each resolving the
                // field path from its DECLARED kind — the second overwrote the
                // first and the `none` came back as `option<string>`.
                "FallThrough",
                "LET $u = (SELECT note FROM ONLY user LIMIT 1);\n\
                 IF $u = NONE THEN THROW 'no user' END;\n\
                 IF $u.note = NONE THEN THROW 'unset' ELSE IF $u.note = NULL THEN THROW 'cleared' END;\n\
                 LET $r = $u.note;",
                "string",
            ),
            (
                // An ordering guard narrowed the row side and not the param
                // side, because only the row recognizer had one. One atom, both
                // sides. `option<int>` before.
                "Ord",
                "LET $x = (SELECT VALUE age FROM ONLY user LIMIT 1);\n\
                 LET $r = IF $x > 18 THEN $x ELSE 0 END;",
                "int",
            ),
        ];

        for (case, query, expected) in cases {
            assert_eq!(
                (case, response_kind(SCHEMA, query).as_str()),
                (case, expected)
            );
        }
    }

    // --- Narrowing-aware guard verdicts ------------------------------------

    use crate::expression::{ExpressionFact, ExpressionValueClass};
    use surrealguard_syntax::span::{ByteRange, SourceSpan};

    /// An env binding the bare param `$name` to `kind` and marking it as
    /// **flow-narrowed** — the state a prior guard leaves behind, which is what
    /// unlocks a dead-branch verdict. (A base binding is [`env_base`].)
    fn env_with(name: &str, kind: Kind) -> StatementEnv {
        let mut env = env_base(name, kind);
        env.mark_param_narrowed(name.into());
        env
    }

    /// An env binding `$name` to `kind` as its **base** declared/seeded binding,
    /// without any flow narrowing — a verdict must never be drawn from this.
    fn env_base(name: &str, kind: Kind) -> StatementEnv {
        let mut env = StatementEnv::default();
        let span = SourceSpan::new(SourceId::new("narrow:test"), ByteRange::new(0, 1).unwrap());
        let fact = ExpressionFact::new(span, ExpressionValueClass::Variable).with_kind(kind);
        env.define_let(name.into(), fact);
        env
    }

    fn record(table: &str) -> Kind {
        Kind::Record(vec![Table::from(table)])
    }

    /// The verdict of the guard in `IF <guard> { ... }`, against `env` — drawn
    /// exactly as [`branch_reachability_in_env`] draws it.
    fn verdict(guard: &str, env: &StatementEnv) -> Verdict {
        let query = format!("RETURN {guard};");
        // A bare boolean call guard (`type::is_record(...)`) lowers as a call;
        // every other guard here is a comparison (binary) expression.
        let expr = if guard.starts_with("type::is_record") {
            call_cond(&query)
        } else {
            cond(&query)
        };
        guard_of(&expr, true, Some(env)).verdict(&EnvOracle(env))
    }

    #[test]
    fn none_guard_verdict_folds_against_the_narrowed_kind() {
        // `$x = NONE` on a non-none record can never hold; `!= NONE` always does.
        let non_none = env_with("x", record("b"));
        assert_eq!(verdict("$x = NONE", &non_none), Verdict::AlwaysFalse);
        assert_eq!(verdict("$x != NONE", &non_none), Verdict::AlwaysTrue);
        assert_eq!(verdict("$x IS NONE", &non_none), Verdict::AlwaysFalse);
        assert_eq!(verdict("$x IS NOT NONE", &non_none), Verdict::AlwaysTrue);

        // A subject that is *only* none folds the other way.
        let only_none = env_with("x", Kind::None);
        assert_eq!(verdict("$x = NONE", &only_none), Verdict::AlwaysTrue);
        assert_eq!(verdict("$x != NONE", &only_none), Verdict::AlwaysFalse);
    }

    #[test]
    fn none_guard_verdict_is_unknown_on_an_optional_subject() {
        // The soundness floor: `option<record>` still might be none — keep it.
        let option = env_with("x", Kind::Either(vec![Kind::None, Kind::Record(vec![])]));
        assert_eq!(verdict("$x = NONE", &option), Verdict::Unknown);
        assert_eq!(verdict("$x != NONE", &option), Verdict::Unknown);
        // An `Any`/open subject is likewise never settled.
        let any = env_with("x", Kind::Any);
        assert_eq!(verdict("$x = NONE", &any), Verdict::Unknown);
        // An unbound subject has no kind → Unknown.
        assert_eq!(
            verdict("$x = NONE", &StatementEnv::default()),
            Verdict::Unknown
        );
    }

    #[test]
    fn null_guard_verdict_distinguishes_none_from_null() {
        // A bare record is neither null nor none: `= NULL` false, `!= NULL` true.
        let non_null = env_with("x", record("b"));
        assert_eq!(verdict("$x = NULL", &non_null), Verdict::AlwaysFalse);
        assert_eq!(verdict("$x != NULL", &non_null), Verdict::AlwaysTrue);
        // A none-only subject is *not* null (`NONE = NULL` is FALSE).
        let only_none = env_with("x", Kind::None);
        assert_eq!(verdict("$x = NULL", &only_none), Verdict::AlwaysFalse);
        // A null-only subject folds true.
        let only_null = env_with("x", Kind::Null);
        assert_eq!(verdict("$x = NULL", &only_null), Verdict::AlwaysTrue);
    }

    #[test]
    fn table_discriminant_verdict_folds_a_pinned_record() {
        // `type::table($x) = 'a'` on a `record<b>` can never hold.
        let rec_b = env_with("x", record("b"));
        assert_eq!(
            verdict("type::table($x) = 'a'", &rec_b),
            Verdict::AlwaysFalse
        );
        assert_eq!(
            verdict("type::table($x) != 'a'", &rec_b),
            Verdict::AlwaysTrue
        );
        // Exactly `{a}` → always true; `!= 'a'` → always false.
        let rec_a = env_with("x", record("a"));
        assert_eq!(
            verdict("type::table($x) = 'a'", &rec_a),
            Verdict::AlwaysTrue
        );
        assert_eq!(
            verdict("type::table($x) != 'a'", &rec_a),
            Verdict::AlwaysFalse
        );
        // An open union `record<a | b>` is not pinned → Unknown either way.
        let union = env_with("x", Kind::Record(vec![Table::from("a"), Table::from("b")]));
        assert_eq!(verdict("type::table($x) = 'a'", &union), Verdict::Unknown);
        // An unconstrained `record<>` is not a pinned union → Unknown.
        let open = env_with("x", Kind::Record(vec![]));
        assert_eq!(verdict("type::table($x) = 'a'", &open), Verdict::Unknown);
    }

    #[test]
    fn is_record_verdict_mirrors_the_table_discriminant() {
        let rec_b = env_with("x", record("b"));
        assert_eq!(
            verdict("type::is_record($x, 'a')", &rec_b),
            Verdict::AlwaysFalse
        );
        let rec_a = env_with("x", record("a"));
        assert_eq!(
            verdict("type::is_record($x, 'a')", &rec_a),
            Verdict::AlwaysTrue
        );
        // A none-carrying subject is not a pinned record union → Unknown (and
        // indeed `is_record` genuinely varies: false on NONE, true on record<a>).
        let opt_a = env_with("x", Kind::Either(vec![Kind::None, record("a")]));
        assert_eq!(
            verdict("type::is_record($x, 'a')", &opt_a),
            Verdict::Unknown
        );
    }

    #[test]
    fn constant_guards_still_fold_without_any_env() {
        // The const path is consulted first, so literal guards are unchanged.
        let empty = StatementEnv::default();
        assert_eq!(verdict("1 == 1", &empty), Verdict::AlwaysTrue);
        assert_eq!(verdict("2 > 3", &empty), Verdict::AlwaysFalse);
        assert_eq!(verdict("$x > 3", &empty), Verdict::Unknown);
    }

    #[test]
    fn a_base_binding_is_never_folded() {
        // The refinement's core rule: a subject at its BASE declared binding —
        // never touched by flow narrowing — yields no verdict, even though the
        // kind technically excludes none / isn't the table. This is what keeps
        // an idiomatic defensive `IF $organization = NONE` on a declared
        // `record<organization>` param from being greyed.
        let base = env_base("x", record("b"));
        assert_eq!(verdict("$x = NONE", &base), Verdict::Unknown);
        assert_eq!(verdict("$x != NONE", &base), Verdict::Unknown);
        assert_eq!(verdict("$x = NULL", &base), Verdict::Unknown);
        assert_eq!(verdict("type::table($x) = 'a'", &base), Verdict::Unknown);
        assert_eq!(verdict("type::is_record($x, 'a')", &base), Verdict::Unknown);
        // A literal-constant guard still folds — it does not depend on a subject.
        assert_eq!(verdict("1 == 1", &base), Verdict::AlwaysTrue);
    }

    #[test]
    fn reachability_in_env_greys_dead_branches_like_the_const_path() {
        use crate::analyzer::const_eval::BranchReach;
        let rec_b = env_with("x", record("b"));

        // A provably-false first guard is DeadFalse; the ELSE survives.
        let parsed = parse_source(
            SourceId::new("narrow:test"),
            "IF $x = NONE { RETURN 1 } ELSE { RETURN 2 };",
        )
        .expect("parses");
        let ast::Statement::IfElse(stmt) =
            surrealguard_syntax::lower::lower_first_statement(&parsed, "IfElseStatement")
                .expect("if statement")
                .node
        else {
            panic!("expected if");
        };
        let reach = branch_reachability_in_env(&stmt, &rec_b);
        assert_eq!(reach.branches, vec![BranchReach::DeadFalse]);
        assert!(!reach.else_dead);

        // A provably-true first guard makes the ELSE dead.
        let parsed = parse_source(
            SourceId::new("narrow:test"),
            "IF $x != NONE { RETURN 1 } ELSE { RETURN 2 };",
        )
        .expect("parses");
        let ast::Statement::IfElse(stmt) =
            surrealguard_syntax::lower::lower_first_statement(&parsed, "IfElseStatement")
                .expect("if statement")
                .node
        else {
            panic!("expected if");
        };
        let reach = branch_reachability_in_env(&stmt, &rec_b);
        assert_eq!(reach.branches, vec![BranchReach::Reachable]);
        assert!(reach.else_dead);
    }
}
