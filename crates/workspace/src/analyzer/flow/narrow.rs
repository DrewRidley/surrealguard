//! Control-flow type narrowing.
//!
//! A guard condition partitions a param's value space; each reachable region
//! downstream sees the param narrowed to the kind consistent with the guard.
//! This module recognizes two guard shapes on a param/`LET`-bound `$x`:
//!
//! - **Sentinel guards** — `$x = NONE`, `$x IS NONE`, `$x != NONE`,
//!   `$x IS NOT NONE`, and the same four against `NULL`. Each eliminates
//!   **only its own sentinel**: `NULL = NONE` is FALSE on the engine, so a
//!   `!= NONE` guard leaves `null` on the table (and `!= NULL` leaves `none`).
//!   The positive form narrows `$x`; the negative form is the complement.
//! - **Discriminant guards** — `type::table($x) = 'lit'` / `!= 'lit'` on a
//!   `record<...>` union. The positive `=` narrows to `record<lit>`; the
//!   negative removes `lit` from the union.
//!
//! The extracted refinements ([`Effect`]s) are applied to the environment so
//! every downstream read of `$x` (function-argument checks, field access)
//! sees the narrowed kind. This generalizes the single-expression
//! `= NONE OR` / `!= NONE AND` narrowing (`infer::none_guarded_param`) to
//! statement and branch flow.

use std::cell::Cell;
use std::sync::OnceLock;

use surrealdb_types::{Kind, Table};
use surrealguard_syntax::ast;
use surrealguard_syntax::span::ByteRange;

use crate::analyzer::const_eval::{BranchReach, Reachability};
use crate::analyzer::context::AnalysisContext;
use crate::analyzer::facts::{
    eval, guard_of, Bindings, ConstValue, DiscriminantKind, KindOracle, NoOracle, Place, PlaceRoot,
    Refinement, Term,
};
use crate::analyzer::facts::place_of;
use crate::analyzer::expression::infer::narrow_out_none;
use crate::statement_env::StatementEnv;

// ---------------------------------------------------------------------------
// The fact-layer gate
// ---------------------------------------------------------------------------

/// Whether the [expression-fact layer](crate::analyzer::facts) answers the
/// narrowing questions, or the hand-written recognizers below do.
///
/// Both paths are compiled, and the answer is read at run time rather than at
/// compile time so one process can exercise both — which is what
/// `tests/fact_layer.rs` does to prove the new path is never *wider* than the
/// old one at any site in the corpus. `SG_FACT_LAYER=1` selects the fact layer
/// for a whole run; [`with_fact_layer`] selects it for one scope.
///
/// This is scaffolding with a stated end: when every consumer reads `Facts`
/// directly the old recognizers go, and the gate goes with them.
pub fn use_fact_layer() -> bool {
    OVERRIDE.with(Cell::get).unwrap_or_else(env_default)
}

thread_local! {
    /// A scoped override, per thread: analysis is single-threaded, and a test
    /// that flips the gate must not flip it for a test running beside it.
    static OVERRIDE: Cell<Option<bool>> = const { Cell::new(None) };
}

fn env_default() -> bool {
    static DEFAULT: OnceLock<bool> = OnceLock::new();
    *DEFAULT.get_or_init(|| {
        matches!(
            std::env::var("SG_FACT_LAYER").as_deref(),
            Ok("1" | "true" | "on")
        )
    })
}

/// Runs `f` with the gate forced one way, restoring the previous setting after
/// — including on unwind, so a failing assertion inside cannot leak the
/// setting into the next test.
pub fn with_fact_layer<T>(enabled: bool, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<bool>);
    impl Drop for Restore {
        fn drop(&mut self) {
            OVERRIDE.with(|slot| slot.set(self.0));
        }
    }
    let _restore = Restore(OVERRIDE.with(|slot| slot.replace(Some(enabled))));
    f()
}

/// A refinement a guard proves about a param at a program point.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Narrowing {
    /// The value IS NONE — it can be nothing else.
    None,
    /// The value IS NULL — it can be nothing else. `NULL` and `NONE` are
    /// distinct values (`NULL = NONE` is FALSE on the engine), so proving one
    /// says nothing about the other.
    Null,
    /// The param is not NONE *and* not NULL (strip both `none` and `null`).
    /// Used by the `> / >=` ordering guards (`NONE > 18` and `NULL > 18` are
    /// both FALSE) and by `type::is_record`.
    NotNone,
    /// The value is not NONE, but MAY still be NULL — strip `none` only,
    /// **keep `null`**. A NULL value has `is_none() == false`, so it survives a
    /// `!= NONE` guard; stripping `null` here would be unsound.
    StripNone,
    /// The value is not NULL, but MAY still be NONE — strip `null` only,
    /// **keep `none`**. `NONE != NULL` is TRUE, so a NONE value survives.
    NotNull,
    /// The value equals a literal: it becomes that literal's singleton
    /// [`Kind::Literal`], but only when the literal's base kind unifies with
    /// the field's non-none base (otherwise the refinement cannot apply).
    Eq(Kind),
    /// The param's record narrows to a single table.
    Table(String),
    /// The param's record excludes a table.
    NotTable(String),
    /// The value is a member of a collection, so it has that collection's
    /// element kind (occurrence typing over `x IN <collection>`). The value is
    /// tightened to this kind on the positive branch only.
    Is(Kind),
    /// A refinement computed by the [expression-fact layer](crate::analyzer::facts):
    /// a composition of lattice meets and subtractions rather than one of the
    /// fixed transforms above.
    ///
    /// This is the whole of the adapter. Every consumer keeps reading
    /// `Narrowing` and applying it through [`narrow_kind`]; only where the
    /// refinement *comes from* changes with the gate. When the consumers read
    /// `Facts` directly (stage 4) the other variants go and this one stops
    /// being a variant at all.
    Refine(Refinement),
}

/// The target of a narrowing: a bare param (`$x`) or a param plus a short
/// field path (`$file.folder`, `$a.b.c`). Bare params rebind their binding;
/// field paths record a per-path override keyed on the exact `param.field…`
/// string — only that path narrows, never the base param or a sibling.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GuardPath {
    /// The base param name (without `$`).
    pub param: String,
    /// The field segments after the param; empty means a bare param.
    pub fields: Vec<String>,
}

impl GuardPath {
    /// A bare-param target.
    pub(crate) fn bare(param: String) -> Self {
        Self {
            param,
            fields: Vec::new(),
        }
    }

    /// Whether this targets the bare param (no field path).
    pub(crate) fn is_bare(&self) -> bool {
        self.fields.is_empty()
    }

    /// The `param.field.field` lookup key (just `param` when bare).
    pub(crate) fn key(&self) -> String {
        if self.fields.is_empty() {
            self.param.clone()
        } else {
            format!("{}.{}", self.param, self.fields.join("."))
        }
    }
}

/// A `(path, refinement)` a guard yields for one param or field path.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Effect {
    /// The narrowed target (bare param or field path).
    pub path: GuardPath,
    /// The refinement to apply to it.
    pub narrowing: Narrowing,
}

/// The refinements that hold in the region where `cond` is TRUE (an `IF`'s
/// THEN body). `env` resolves indirect discriminants (a `LET`-bound
/// `type::table($x)`).
pub(crate) fn positive_effects(cond: &ast::Expr, env: &StatementEnv) -> Vec<Effect> {
    if use_fact_layer() {
        return fact_effects(cond, true, env);
    }
    effects(cond, true, env)
}

/// The refinements that hold in the region where `cond` is FALSE (an `IF`'s
/// ELSE body, or the fall-through after a diverging guard).
pub(crate) fn negative_effects(cond: &ast::Expr, env: &StatementEnv) -> Vec<Effect> {
    if use_fact_layer() {
        return fact_effects(cond, false, env);
    }
    effects(cond, false, env)
}

/// [`positive_effects`]/[`negative_effects`], answered by the fact layer.
///
/// One guard, interpreted once, projected back into the `Vec<Effect>` shape the
/// consumers already read. Only param-rooted places survive the projection —
/// an `Effect` is keyed by [`GuardPath`], and a bare row field is the `WHERE`
/// side's business ([`where_effects`]), exactly as it was when the two sides
/// were two recognizers.
fn fact_effects(cond: &ast::Expr, positive: bool, env: &StatementEnv) -> Vec<Effect> {
    guard_of(cond, positive, Some(env))
        .facts(&EnvOracle(env))
        .iter()
        .filter_map(|(place, refinement)| {
            Some(Effect {
                path: param_path(place)?,
                narrowing: Narrowing::Refine(refinement.clone()),
            })
        })
        .collect()
}

/// The kinds the flow environment holds, as the fact layer reads them.
///
/// A bare param resolves to its binding; a field path resolves only when an
/// earlier narrowing recorded one. The *declared* kind of a field path is not
/// available here — stepping it needs the schema, which the effect callers do
/// not pass — so a place it cannot answer is answered `None`, and the atoms
/// that need a kind (membership) simply prove nothing. That is the same
/// coverage the recognizer it replaces had, for the same reason.
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
}

// ---------------------------------------------------------------------------
// WHERE-narrowing of SELECT result rows (design §3.1)
// ---------------------------------------------------------------------------

/// A refinement a SELECT's `WHERE` provably makes about a **projected row
/// field**, keyed by its schema field path (`["email"]`, `["profile","email"]`)
/// rather than a param binding. Consumed by the SELECT result-type post-pass,
/// which tightens the matching leaf of the projected object literal.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RowEffect {
    /// The row field path the guard tightens.
    pub fields: Vec<String>,
    /// The refinement to apply at that path.
    pub narrowing: Narrowing,
}

/// The refinements a SELECT's `WHERE` proves about the returned rows. Every
/// surviving row satisfies the predicate, so this is a single **positive**
/// flow-guard: `A AND B` contributes the union of both sides' effects (both
/// hold on every row), `A OR B` contributes nothing (either disjunct may be
/// the reason a row survives), and each recognized leaf yields one effect.
///
/// This mirrors the positive branch of [`effects`], but resolves **bare row
/// fields** (`email`, not `$param`) rather than param bindings, so it produces
/// [`RowEffect`]s keyed by field path.
pub(crate) fn where_effects(cond: &ast::Expr) -> Vec<RowEffect> {
    if use_fact_layer() {
        // The same guard, the same interpreter — with no environment, because
        // a `WHERE` narrows the row it filters and there is no binding to
        // resolve. Only row-rooted places survive: a `WHERE $p != NONE` says
        // nothing about the projected row.
        return guard_of(cond, true, None)
            .facts(&NoOracle)
            .iter()
            .filter_map(|(place, refinement)| {
                if !matches!(place.root, PlaceRoot::RowField) {
                    return None;
                }
                Some(RowEffect {
                    fields: place.field_path()?,
                    narrowing: Narrowing::Refine(refinement.clone()),
                })
            })
            .collect();
    }
    match cond {
        ast::Expr::Binary { lhs, op, rhs } => match &op.node {
            ast::BinaryOp::And => {
                let mut effects = where_effects(&lhs.node);
                effects.extend(where_effects(&rhs.node));
                effects
            }
            // `A OR B` narrows nothing in P1 — a surviving row need only satisfy
            // one disjunct, so neither is a fact about the whole result set.
            ast::BinaryOp::Or => Vec::new(),
            op => row_leaf_effect(&lhs.node, op, &rhs.node)
                .into_iter()
                .collect(),
        },
        _ => Vec::new(),
    }
}

/// The [`RowEffect`] a single WHERE comparison proves, if it is a recognized
/// leaf shape. The order tries the most specific idioms first so a
/// `type::table(f) = 'tbl'` discriminant is never mistaken for a literal-eq.
fn row_leaf_effect(lhs: &ast::Expr, op: &ast::BinaryOp, rhs: &ast::Expr) -> Option<RowEffect> {
    row_none_null_effect(lhs, op, rhs)
        .or_else(|| row_table_effect(lhs, op, rhs))
        .or_else(|| row_literal_eq_effect(lhs, op, rhs))
        .or_else(|| row_order_effect(lhs, op, rhs))
}

/// `f != NONE` / `f IS NOT NONE` → `StripNone` (keep `null`);
/// `f != NULL` / `f IS NOT NULL` → `NotNull` (keep `none`). Only the
/// not-equals polarity narrows: `f = NONE` is not a recognized tightening (it
/// is absent from the design table), so it is a no-op.
fn row_none_null_effect(lhs: &ast::Expr, op: &ast::BinaryOp, rhs: &ast::Expr) -> Option<RowEffect> {
    // `Some(false)` is the `!=` / `IS NOT` polarity; `= NONE` (Some(true)) and
    // non-none comparisons narrow nothing.
    if none_test_polarity(op) != Some(false) {
        return None;
    }
    let (fields, is_none) = if let Some(is_none) = none_or_null_literal(rhs) {
        (row_field_path(lhs)?, is_none)
    } else if let Some(is_none) = none_or_null_literal(lhs) {
        (row_field_path(rhs)?, is_none)
    } else {
        return None;
    };
    let narrowing = if is_none {
        Narrowing::StripNone
    } else {
        Narrowing::NotNull
    };
    Some(RowEffect { fields, narrowing })
}

/// `f = <lit>` (either operand order) → `Eq(literal_kind)`. The unifiability of
/// the literal against `f` is checked when the effect is applied (`narrow_kind`
/// returns `None` when it cannot tighten), so a mistyped `f = <lit>` is a
/// no-op rather than a wrong type.
fn row_literal_eq_effect(
    lhs: &ast::Expr,
    op: &ast::BinaryOp,
    rhs: &ast::Expr,
) -> Option<RowEffect> {
    if !matches!(op, ast::BinaryOp::Eq) {
        return None;
    }
    let (fields, literal) = if let Some(literal) = eq_literal_kind(rhs) {
        (row_field_path(lhs)?, literal)
    } else if let Some(literal) = eq_literal_kind(lhs) {
        (row_field_path(rhs)?, literal)
    } else {
        return None;
    };
    Some(RowEffect {
        fields,
        narrowing: Narrowing::Eq(literal),
    })
}

/// `type::table(f) = 'tbl'` → `Table` (narrow the record union to `tbl`);
/// `type::table(f) != 'tbl'` → `NotTable` (remove `tbl`). Either operand order.
fn row_table_effect(lhs: &ast::Expr, op: &ast::BinaryOp, rhs: &ast::Expr) -> Option<RowEffect> {
    if !matches!(op, ast::BinaryOp::Eq | ast::BinaryOp::NotEq) {
        return None;
    }
    let of = |disc: &ast::Expr, lit: &ast::Expr| {
        Some((row_type_table_field(disc)?, string_literal(lit)?))
    };
    let (fields, table) = of(lhs, rhs).or_else(|| of(rhs, lhs))?;
    let narrowing = if matches!(op, ast::BinaryOp::Eq) {
        Narrowing::Table(table)
    } else {
        Narrowing::NotTable(table)
    };
    Some(RowEffect { fields, narrowing })
}

/// `f > lit` / `f >= lit` → `NotNone` (strip `none`+`null`: `NONE > 18` is
/// FALSE, so NONE/NULL rows are filtered). `f < lit` / `f <= lit` narrows
/// **nothing** — `NONE < 65` is TRUE (NONE is the lowest discriminant), so
/// NONE rows survive and stripping the option would be unsound. Operand order
/// is normalized: `18 < f` is treated as `f > 18`.
fn row_order_effect(lhs: &ast::Expr, op: &ast::BinaryOp, rhs: &ast::Expr) -> Option<RowEffect> {
    // Require one side a row field and the other an ordering literal, then
    // re-orient the operator so the field is on the left.
    let (fields, effective_op) = if let Some(fields) = row_field_path(lhs) {
        if !is_ordering_literal(rhs) {
            return None;
        }
        (fields, op.clone())
    } else if let Some(fields) = row_field_path(rhs) {
        if !is_ordering_literal(lhs) {
            return None;
        }
        (fields, flip_order(op)?)
    } else {
        return None;
    };
    match effective_op {
        ast::BinaryOp::Gt | ast::BinaryOp::GtEq => Some(RowEffect {
            fields,
            narrowing: Narrowing::NotNone,
        }),
        // `<` / `<=` on the field side: NONE survives, so narrow nothing.
        _ => None,
    }
}

/// The field-relative operator when the field is on the *right* of a
/// comparison: `18 < f` is `f > 18`. Only the four ordering operators flip.
fn flip_order(op: &ast::BinaryOp) -> Option<ast::BinaryOp> {
    Some(match op {
        ast::BinaryOp::Gt => ast::BinaryOp::Lt,
        ast::BinaryOp::GtEq => ast::BinaryOp::LtEq,
        ast::BinaryOp::Lt => ast::BinaryOp::Gt,
        ast::BinaryOp::LtEq => ast::BinaryOp::GtEq,
        _ => return None,
    })
}

/// The bare row-field path an expression names (`email`, `profile.email`).
/// A param-rooted expression (`$x.f`) names a param place, not a row field.
fn row_field_path(expr: &ast::Expr) -> Option<Vec<String>> {
    let place = place_of(expr)?;
    matches!(place.root, PlaceRoot::RowField)
        .then(|| place.field_path())
        .flatten()
}

/// The row-field path a `type::table(<field>)` call discriminates.
fn row_type_table_field(expr: &ast::Expr) -> Option<Vec<String>> {
    let Term::Discriminant {
        of,
        kind: DiscriminantKind::RecordTable,
    } = eval(expr, Bindings::NONE)
    else {
        return None;
    };
    matches!(of.root, PlaceRoot::RowField)
        .then(|| of.field_path())
        .flatten()
}

/// `Some(true)` for the `NONE` sentinel, `Some(false)` for `NULL`, else `None`.
///
/// One of the four recognizers §1.5 of the fact-layer design counted; the
/// other three keep their own *policies* (which sentinel narrows what) but
/// now read the same denotation, so they can no longer disagree about which
/// sentinel a comparison names.
fn none_or_null_literal(expr: &ast::Expr) -> Option<bool> {
    match eval(expr, Bindings::NONE) {
        Term::Const(ConstValue::None) => Some(true),
        Term::Const(ConstValue::Null) => Some(false),
        _ => None,
    }
}

/// The singleton [`Kind::Literal`] a comparable scalar constant denotes, for
/// the `f = <lit>` refinement. NONE/NULL and non-representable literals
/// (datetime, uuid, regex) have no singleton kind, so they narrow nothing.
fn eq_literal_kind(expr: &ast::Expr) -> Option<Kind> {
    match eval(expr, Bindings::NONE) {
        Term::Const(value) => value.singleton_kind(),
        _ => None,
    }
}

/// Whether an expression is a comparable ordering literal (any literal but
/// `NONE`/`NULL`, which are the low discriminants the ordering rules turn on).
fn is_ordering_literal(expr: &ast::Expr) -> bool {
    matches!(
        expr,
        ast::Expr::Literal(literal)
            if !matches!(literal, ast::Literal::None | ast::Literal::Null)
    )
}

/// The refinements a condition proves under the requested polarity. Compound
/// conditions distribute soundly: only the disjunct/conjunct combinations
/// that pin a definite fact contribute — `A OR B` narrows nothing when true
/// (either disjunct might be the reason) but yields `¬A ∧ ¬B` when false, and
/// `A AND B` is the mirror.
fn effects(cond: &ast::Expr, positive: bool, env: &StatementEnv) -> Vec<Effect> {
    match cond {
        ast::Expr::Binary { lhs, op, rhs } => match &op.node {
            ast::BinaryOp::Or => {
                if positive {
                    Vec::new()
                } else {
                    let mut result = effects(&lhs.node, false, env);
                    result.extend(effects(&rhs.node, false, env));
                    result
                }
            }
            ast::BinaryOp::And => {
                if positive {
                    let mut result = effects(&lhs.node, true, env);
                    result.extend(effects(&rhs.node, true, env));
                    result
                } else {
                    Vec::new()
                }
            }
            op => leaf_effect(&lhs.node, op, &rhs.node, positive, env)
                .or_else(|| in_effect(&lhs.node, op, &rhs.node, positive, env))
                .into_iter()
                .collect(),
        },
        // A bare boolean guard call: `type::is_record($x)` /
        // `type::is_record($x, 'tbl')`. Positive-only (see `is_record_effect`).
        ast::Expr::Call(call) => is_record_effect(call, positive).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// The refinement a `type::is_record` guard proves. This mirrors the
/// `type::table(...) = 'lit'` discriminant, but the predicate is the *call
/// itself* used as a boolean (not a comparison), and it narrows on the
/// **positive branch only**: `type::is_record($x)` is FALSE when `$x` is NONE,
/// a non-record scalar, or (with a table arg) a record of another table, so a
/// false result proves nothing about the kind.
///
/// - `type::is_record($x)`        → `NotNone` (strip the `none`, leaving the
///   non-none open `record`).
/// - `type::is_record($x, 'tbl')` → `Table("tbl")` (narrow the record union to
///   `record<tbl>`, also dropping the `none`).
///
/// Lowering normalizes `type::is::record` → `type::is_record`
/// (`function/mod.rs:55`), so only the underscore spelling is matched here.
fn is_record_effect(call: &ast::Call, positive: bool) -> Option<Effect> {
    if !positive || call.path.node != "type::is_record" {
        return None;
    }
    let path = guard_path_of(&call.args.first()?.node)?;
    let narrowing = match call.args.get(1) {
        Some(table_arg) => Narrowing::Table(string_literal(&table_arg.node)?),
        None => Narrowing::NotNone,
    };
    Some(Effect { path, narrowing })
}

/// The refinement a single comparison proves under `positive`, if it is a
/// recognized guard shape.
fn leaf_effect(
    lhs: &ast::Expr,
    op: &ast::BinaryOp,
    rhs: &ast::Expr,
    positive: bool,
    env: &StatementEnv,
) -> Option<Effect> {
    // Sentinel guard: `$x = NONE` / `$x IS NONE` / `$x != NONE` /
    // `$x IS NOT NONE` and the same four against `NULL`, on a bare param or a
    // simple field path (`$file.folder != NONE`). This is the same recognizer
    // the row/WHERE side uses (`row_none_null_effect`) and the same recognizer
    // the dead-branch verdicts use — one notion of "which sentinel".
    if let Some(equals) = none_test_polarity(op) {
        if let Some((path, is_none)) = none_or_null_guard(lhs, rhs) {
            // `= <sentinel>` true ⇒ the value IS that sentinel; `!=` true ⇒ it
            // is not — and eliminates ONLY that one. `NULL = NONE` is FALSE on
            // the engine, so `$x != NONE` on an `option<string | null>` still
            // leaves `null` reachable; dropping it here shipped a wrong type.
            let when_true = if equals {
                sentinel_is(is_none)
            } else {
                sentinel_is_not(is_none)
            };
            let narrowing = if positive { when_true } else { when_true.flip_sentinel() };
            return Some(Effect { path, narrowing });
        }
    }
    // Discriminant guard: `type::table($x) = 'lit'` / `!= 'lit'`, either the
    // direct call form or a `LET`-bound `$table = 'lit'` (indirect).
    if matches!(op, ast::BinaryOp::Eq | ast::BinaryOp::NotEq) {
        if let Some((path, table)) = table_discriminant(lhs, rhs, env) {
            let is_eq = matches!(op, ast::BinaryOp::Eq);
            // `= 'lit'` true ⇒ Table(lit); `!= 'lit'` true ⇒ NotTable(lit).
            let when_true = if is_eq {
                Narrowing::Table(table.clone())
            } else {
                Narrowing::NotTable(table.clone())
            };
            let narrowing = if positive {
                when_true
            } else {
                when_true.flip_table(&table)
            };
            return Some(Effect { path, narrowing });
        }
    }
    None
}

/// The refinement a `subject IN <collection>` membership guard proves:
/// `subject` is a member of the collection, so on the **positive branch** it
/// has the collection's element kind (occurrence typing). The negative branch
/// narrows nothing — "not in" tells you nothing about the value's kind, so it
/// is positive-only like [`is_record_effect`].
///
/// `subject` is a bare param (`$x`) or a simple field path (`$a.b`), reusing
/// [`guard_path_of`]. The collection's element kind must be determinable and
/// concrete — an undeterminable collection or an `Any` element (a bare
/// `array`/`set`) yields no effect, so narrowing never rests on uncertainty.
fn in_effect(
    lhs: &ast::Expr,
    op: &ast::BinaryOp,
    rhs: &ast::Expr,
    positive: bool,
    env: &StatementEnv,
) -> Option<Effect> {
    if !positive {
        return None;
    }
    // `IN` (any case) is the only membership operator that narrows the element:
    // `CONTAINS` puts the collection on the left, and `INSIDE` is an alias whose
    // element-narrowing is not needed here.
    let ast::BinaryOp::Other(name) = op else {
        return None;
    };
    if !name.eq_ignore_ascii_case("IN") {
        return None;
    }
    let path = guard_path_of(lhs)?;
    let element = collection_element_kind(rhs, env)?;
    // Never narrow on an undeterminable element.
    if matches!(element, Kind::Any) {
        return None;
    }
    Some(Effect {
        path,
        narrowing: Narrowing::Is(element),
    })
}

/// The element kind of a collection expression, resolved through `env`: a bare
/// bound param that holds an `array<E>`/`set<E>`. Only a bare param resolves
/// here — this recognizer runs without a schema, so a field-path collection
/// (which would need schema stepping) is deliberately left alone.
fn collection_element_kind(expr: &ast::Expr, env: &StatementEnv) -> Option<Kind> {
    let place = place_of(expr)?;
    let PlaceRoot::Param(name) = &place.root else {
        return None;
    };
    if !place.path.is_empty() {
        return None;
    }
    match env.let_fact(name)?.kind.clone()? {
        Kind::Array(element, _) | Kind::Set(element, _) => Some(*element),
        _ => None,
    }
}

/// "The value IS this sentinel" — `NONE` when `is_none`, else `NULL`.
pub(crate) fn sentinel_is(is_none: bool) -> Narrowing {
    if is_none {
        Narrowing::None
    } else {
        Narrowing::Null
    }
}

/// "The value is NOT this sentinel" — and **only** this one. `NONE` and `NULL`
/// are distinct values on the engine (`NULL = NONE` is FALSE, `NONE IS NULL` is
/// FALSE), so `!= NONE` leaves `null` reachable and `!= NULL` leaves `none`.
pub(crate) fn sentinel_is_not(is_none: bool) -> Narrowing {
    if is_none {
        Narrowing::StripNone
    } else {
        Narrowing::NotNull
    }
}

impl Narrowing {
    /// The refinement the *negation* of this guard proves. Only the sentinel
    /// refinements have a complement worth recording: `= NONE` failing means
    /// the value is not NONE (but may be NULL), and vice versa.
    fn flip_sentinel(self) -> Narrowing {
        match self {
            Narrowing::None => Narrowing::StripNone,
            Narrowing::StripNone => Narrowing::None,
            Narrowing::Null => Narrowing::NotNull,
            Narrowing::NotNull => Narrowing::Null,
            other => other,
        }
    }

    fn flip_table(self, table: &str) -> Narrowing {
        match self {
            Narrowing::Table(_) => Narrowing::NotTable(table.to_string()),
            Narrowing::NotTable(_) => Narrowing::Table(table.to_string()),
            other => other,
        }
    }
}

/// Whether `op` is an equality-family NONE test: `Some(true)` for
/// `=`/`IS` (equals-none), `Some(false)` for `!=`/`IS NOT` (not-equals-none).
fn none_test_polarity(op: &ast::BinaryOp) -> Option<bool> {
    match op {
        ast::BinaryOp::Eq => Some(true),
        ast::BinaryOp::NotEq => Some(false),
        ast::BinaryOp::Other(text) => {
            let words: Vec<&str> = text.split_whitespace().collect();
            match words.as_slice() {
                [a] if a.eq_ignore_ascii_case("IS") => Some(true),
                [a, b] if a.eq_ignore_ascii_case("IS") && b.eq_ignore_ascii_case("NOT") => {
                    Some(false)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The `(subject path, is_none)` a `NONE`/`NULL` comparison names, from either
/// operand order. `is_none` is `true` for the `NONE` sentinel, `false` for
/// `NULL`.
pub(crate) fn none_or_null_guard(lhs: &ast::Expr, rhs: &ast::Expr) -> Option<(GuardPath, bool)> {
    if let Some(is_none) = none_or_null_literal(rhs) {
        Some((guard_path_of(lhs)?, is_none))
    } else if let Some(is_none) = none_or_null_literal(lhs) {
        Some((guard_path_of(rhs)?, is_none))
    } else {
        None
    }
}

/// The narrowable target an expression names: a bare `$param`, or a simple
/// path `$param.field.field` (plain field steps only). Shared by both the
/// statement/branch guards here and the in-expression `!= NONE AND` /
/// `= NONE OR` narrowing (`infer`).
pub(crate) fn guard_path_of(expr: &ast::Expr) -> Option<GuardPath> {
    param_path(&place_of(expr)?)
}

/// A place as a [`GuardPath`], when it is rooted in a param and every step
/// names a field. A subscript ends the path: only the exact written field
/// path is refinable.
fn param_path(place: &Place) -> Option<GuardPath> {
    let PlaceRoot::Param(param) = &place.root else {
        return None;
    };
    Some(GuardPath {
        param: param.clone(),
        fields: place.field_path()?,
    })
}

/// The `(path, table_literal)` of a table discriminant compared to a string
/// literal, from either side. The discriminated side is either a direct
/// `type::table($path)` call or a `LET`-bound `$table` that holds one
/// (resolved through `env`).
fn table_discriminant(
    lhs: &ast::Expr,
    rhs: &ast::Expr,
    env: &StatementEnv,
) -> Option<(GuardPath, String)> {
    let of = |disc_side: &ast::Expr, lit_side: &ast::Expr| {
        let path = discriminated_path(disc_side, env)?;
        let table = string_literal(lit_side)?;
        Some((path, table))
    };
    of(lhs, rhs).or_else(|| of(rhs, lhs))
}

/// The record param/path a discriminant expression tests: a `type::table(...)`
/// call directly, or a `$binding` the env records as one.
fn discriminated_path(expr: &ast::Expr, env: &StatementEnv) -> Option<GuardPath> {
    type_table_path(expr).or_else(|| match expr {
        ast::Expr::Param(binding) => {
            env.table_discriminant(binding).map(|p| GuardPath::bare(p.to_string()))
        }
        _ => None,
    })
}

/// The param path a `type::table(...)` call discriminates.
fn type_table_path(expr: &ast::Expr) -> Option<GuardPath> {
    let Term::Discriminant {
        of,
        kind: DiscriminantKind::RecordTable,
    } = eval(expr, Bindings::NONE)
    else {
        return None;
    };
    param_path(&of)
}

/// The param name, when `expr` is a bare `type::table($param)` call — used to
/// seed the indirect-discriminant map from `LET $t = type::table($param)`.
/// (Only bare params are recorded as indirect discriminants.)
pub(crate) fn type_table_arg(expr: &ast::Expr) -> Option<String> {
    match type_table_path(expr) {
        Some(path) if path.is_bare() => Some(path.param),
        _ => None,
    }
}

/// The string `expr` provably denotes, if it denotes one.
fn string_literal(expr: &ast::Expr) -> Option<String> {
    match eval(expr, Bindings::NONE) {
        Term::Const(value) => value.as_str().map(str::to_string),
        _ => None,
    }
}

/// Applies each effect's refinement to the current scope. A bare param
/// rebinds its binding to the narrowed kind; a field path records a per-path
/// override keyed on the exact `param.field…` string. Unbound bases and
/// refinements that don't tighten the current kind are skipped, so narrowing
/// only ever tightens a known kind — and never touches the base param or a
/// sibling path.
pub(crate) fn apply_effects(ctx: &mut AnalysisContext<'_>, effects: &[Effect]) {
    apply_effects_over(ctx, effects, None);
}

/// [`apply_effects`], additionally recording the source region each refinement
/// holds over.
///
/// The environment alone cannot answer an editor's question — it is a moving
/// point-in-time value, and by the time hover runs, analysis is over. So the
/// callers that *know* the region a refinement covers (a branch body, the
/// statements after a diverging guard) pass it here, and it is recorded
/// alongside the narrowed kind. Callers that don't — the `AND`/`OR`
/// short-circuit inside a single expression — use [`apply_effects`] and record
/// nothing, which leaves the editor showing the declared kind exactly as before.
pub(crate) fn apply_effects_over(
    ctx: &mut AnalysisContext<'_>,
    effects: &[Effect],
    region: Option<ByteRange>,
) {
    for effect in effects {
        let Some(base) = ctx.env().let_fact(&effect.path.param).cloned() else {
            continue;
        };
        let Some(base_kind) = base.kind.clone() else {
            continue;
        };
        if effect.path.is_bare() {
            let Some(narrowed) = narrow_kind(&base_kind, &effect.narrowing) else {
                continue;
            };
            let mut new_fact = base;
            new_fact.kind = Some(narrowed.clone());
            // Mark this as a flow narrowing (not a base binding) so dead-branch
            // folding may draw a verdict from the tightened kind.
            ctx.narrow_local(effect.path.param.clone(), new_fact);
            if let Some(region) = region {
                ctx.record_narrowing(effect.path.param.clone(), region, narrowed);
            }
        } else {
            // Resolve the path's declared kind through the schema, then narrow
            // and record it under the exact path key.
            let Some(current) = crate::analyzer::expression::infer::step_field_path(
                &base_kind,
                &effect.path.fields,
                ctx.schema(),
            ) else {
                continue;
            };
            let Some(narrowed) = narrow_kind(&current, &effect.narrowing) else {
                continue;
            };
            if let Some(region) = region {
                ctx.record_narrowing(effect.path.key(), region, narrowed.clone());
            }
            ctx.define_narrowed_path(effect.path.key(), narrowed);
        }
    }
}

/// The kind `kind` refines to under `narrowing`, or `None` when the
/// refinement leaves it unchanged (or cannot apply).
pub(crate) fn narrow_kind(kind: &Kind, narrowing: &Narrowing) -> Option<Kind> {
    match narrowing {
        Narrowing::None => Some(Kind::None),
        Narrowing::Null => Some(Kind::Null),
        Narrowing::NotNone => {
            let narrowed = narrow_out_none(kind);
            (narrowed != *kind).then_some(narrowed)
        }
        Narrowing::StripNone => {
            let narrowed = strip_variant(kind, Kind::None);
            (narrowed != *kind).then_some(narrowed)
        }
        Narrowing::NotNull => {
            let narrowed = strip_variant(kind, Kind::Null);
            (narrowed != *kind).then_some(narrowed)
        }
        Narrowing::Eq(literal) => eq_narrow(kind, literal),
        Narrowing::Table(table) => narrow_record_to(kind, table),
        Narrowing::NotTable(table) => narrow_record_without(kind, table),
        // Occurrence typing over `x IN <collection>`: on the positive branch the
        // value is a member, so it takes the collection's element kind. Tighten
        // only when this actually changes the kind (and the element is a real
        // kind, not `Any` — guarded against upstream in `in_effect`).
        Narrowing::Is(target) => {
            (*target != Kind::Any && target != kind).then(|| target.clone())
        }
        // The fact layer answers this one itself: a `Refinement` already *is*
        // the function from kind to narrowed kind, and it reports "no
        // tightening" the same way every arm above does.
        Narrowing::Refine(refinement) => refinement.apply(kind),
    }
}

/// Drops a single scalar variant (`Kind::None` or `Kind::Null`) from a union,
/// collapsing a one-variant remainder. A non-union kind is unchanged. Unlike
/// [`narrow_out_none`], this removes ONLY the requested variant, so the other
/// option marker survives (`!= NONE` keeps `null`, `!= NULL` keeps `none`).
fn strip_variant(kind: &Kind, drop: Kind) -> Kind {
    let Kind::Either(variants) = kind else {
        return kind.clone();
    };
    let kept: Vec<Kind> = variants.iter().filter(|v| **v != drop).cloned().collect();
    match kept.len() {
        0 => kind.clone(),
        1 => kept.into_iter().next().expect("one variant"),
        _ => Kind::Either(kept),
    }
}

/// The kind of a field pinned to a literal by `f = <lit>`: the literal's
/// singleton kind, but only when the literal's base kind unifies with the
/// field's non-none base. When it does not (a mistyped comparison), the
/// refinement cannot apply and the field keeps its schema kind.
fn eq_narrow(field: &Kind, literal: &Kind) -> Option<Kind> {
    let literal_base = crate::kinds::literal_base_kind(literal)?;
    let stripped = narrow_out_none(field);
    // Any is the universal top: a literal always unifies with it.
    if stripped == Kind::Any {
        return Some(literal.clone());
    }
    let field_base = crate::kinds::literal_base_kind(&stripped).unwrap_or(stripped);
    (field_base == literal_base).then(|| literal.clone())
}

/// Narrows a `record<...>` (optionally wrapped in a union) to the single
/// `record<table>`, when `table` is among its targets. An unconstrained
/// `record<>` narrows to `record<table>`.
fn narrow_record_to(kind: &Kind, table: &str) -> Option<Kind> {
    match kind {
        Kind::Record(tables) => {
            let present = tables.is_empty() || tables.iter().any(|t| t.to_string() == table);
            present.then(|| Kind::Record(vec![Table::from(table)]))
        }
        Kind::Either(variants) => {
            let narrowed: Vec<Kind> = variants
                .iter()
                .filter_map(|variant| narrow_record_to(variant, table))
                .collect();
            match narrowed.len() {
                0 => None,
                1 => narrowed.into_iter().next(),
                _ => Some(Kind::either(narrowed)),
            }
        }
        _ => None,
    }
}

/// Removes `table` from a `record<...>` union (through an enclosing union),
/// e.g. `record<file | folder>` minus `folder` becomes `record<file>`.
fn narrow_record_without(kind: &Kind, table: &str) -> Option<Kind> {
    match kind {
        Kind::Record(tables) => {
            let kept: Vec<Table> = tables
                .iter()
                .filter(|t| t.to_string() != table)
                .cloned()
                .collect();
            // Nothing removed (an unconstrained `record<>` or absent table)
            // leaves the kind unchanged.
            (!tables.is_empty() && kept.len() != tables.len()).then(|| Kind::Record(kept))
        }
        Kind::Either(variants) => {
            let narrowed: Vec<Kind> = variants
                .iter()
                .map(|variant| narrow_record_without(variant, table).unwrap_or_else(|| variant.clone()))
                .collect();
            Some(Kind::either(narrowed))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Narrowing-aware guard verdicts (dead-branch folding beyond literal constants)
// ---------------------------------------------------------------------------

/// Whether a guard's outcome is provably fixed given the flow-narrowed kinds
/// the env already knows for the guard's subject. This is the narrowing
/// analogue of [`crate::analyzer::const_eval::const_eval_bool`]: where the
/// const folder proves a guard true/false from *literals*, this proves it from
/// *accumulated narrowing* (`$x` already known non-none, a record already
/// pinned to one table, …).
///
/// **Soundness is one-directional.** A branch is greyed only on a `AlwaysFalse`
/// / (later-branch-killing) `AlwaysTrue`, so the recognizers below return a
/// definite verdict *only* when the env kind rules the guard out (or in) for
/// **every** value the subject can still take. An `Unknown`/`Any`/still-
/// possible kind yields [`Verdict::Unknown`] and keeps the branch. Greying a
/// live branch is a real false positive; missing a dead one is merely
/// incomplete — so every recognizer bails toward `Unknown`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// The guard holds for every value the subject can now be.
    AlwaysTrue,
    /// The guard holds for no value the subject can now be.
    AlwaysFalse,
    /// The guard's outcome is not fixed — the branch must be kept.
    Unknown,
}

impl Verdict {
    /// The verdict of the negation of this guard (`!=` vs `=`, ELSE of a THEN).
    fn negate(self) -> Verdict {
        match self {
            Verdict::AlwaysTrue => Verdict::AlwaysFalse,
            Verdict::AlwaysFalse => Verdict::AlwaysTrue,
            Verdict::Unknown => Verdict::Unknown,
        }
    }
}

/// The verdict of an `IF`/`ELSE IF` guard `cond` against the kinds `env`
/// currently holds for its subject. Literal-constant guards fold first (the
/// pure const path), so this preserves the constant-folding behavior exactly
/// and only *adds* the narrowing-driven verdicts as a fallback.
///
/// Recognized narrowing shapes (mirroring [`effects`]/[`leaf_effect`]):
/// - `$x = NONE` / `$x != NONE` (and `IS`/`IS NOT`), likewise `= NULL`;
/// - `type::table($x) = 'tbl'` / `!= 'tbl'` on a record union;
/// - `type::is_record($x, 'tbl')` used as a bare boolean guard.
///
/// Anything else — a compound `AND`/`OR` the const path could not decide, an
/// unbound subject, an `Any`/open kind — is [`Verdict::Unknown`].
pub(crate) fn guard_verdict(cond: &ast::Expr, env: &StatementEnv) -> Verdict {
    // Literal-constant guards fold first, reusing the pure const path so the
    // existing constant behavior is preserved verbatim.
    if let Some(value) = crate::analyzer::const_eval::const_eval_bool(cond) {
        return if value {
            Verdict::AlwaysTrue
        } else {
            Verdict::AlwaysFalse
        };
    }
    match cond {
        ast::Expr::Binary { lhs, op, rhs } => binary_verdict(&lhs.node, &op.node, &rhs.node, env),
        // A bare boolean guard call: `type::is_record($x, 'tbl')`.
        ast::Expr::Call(call) => is_record_verdict(call, env),
        _ => Verdict::Unknown,
    }
}

/// The verdict of a single comparison guard under the env's narrowed kinds.
/// Compound `AND`/`OR` guards are intentionally not decomposed here — only the
/// const path (short-circuit) decides those — so a comparison the const folder
/// could not settle is the only narrowing entry point.
fn binary_verdict(
    lhs: &ast::Expr,
    op: &ast::BinaryOp,
    rhs: &ast::Expr,
    env: &StatementEnv,
) -> Verdict {
    // NONE / NULL equality guards: `$x = NONE` / `$x != NULL` / `$x IS NONE`…
    if let Some(equals) = none_test_polarity(op) {
        if let Some((path, is_none)) = none_or_null_guard(lhs, rhs) {
            let Some(kind) = path_kind(&path, env) else {
                return Verdict::Unknown;
            };
            let eq_verdict = sentinel_eq_verdict(&kind, is_none);
            // `equals` is the `=`/`IS` polarity; `!=`/`IS NOT` negates it.
            return if equals { eq_verdict } else { eq_verdict.negate() };
        }
    }
    // Discriminant guard: `type::table($x) = 'tbl'` / `!= 'tbl'`, either the
    // direct call form or a `LET`-bound indirect discriminant.
    if matches!(op, ast::BinaryOp::Eq | ast::BinaryOp::NotEq) {
        if let Some((path, table)) = table_discriminant(lhs, rhs, env) {
            let Some(kind) = path_kind(&path, env) else {
                return Verdict::Unknown;
            };
            let eq_verdict = table_eq_verdict(&kind, &table);
            return if matches!(op, ast::BinaryOp::Eq) {
                eq_verdict
            } else {
                eq_verdict.negate()
            };
        }
    }
    Verdict::Unknown
}

/// The verdict of `type::is_record($x, 'tbl')` as a bare boolean guard: the
/// same record-union discriminant as `type::table($x) = 'tbl'`. The bare
/// `type::is_record($x)` (no table) is not settled here — it also turns FALSE
/// on NONE/scalars, so a pure verdict would need more than the record union.
fn is_record_verdict(call: &ast::Call, env: &StatementEnv) -> Verdict {
    if call.path.node != "type::is_record" {
        return Verdict::Unknown;
    }
    let Some(path) = call.args.first().and_then(|arg| guard_path_of(&arg.node)) else {
        return Verdict::Unknown;
    };
    let Some(table_arg) = call.args.get(1) else {
        return Verdict::Unknown;
    };
    let Some(table) = string_literal(&table_arg.node) else {
        return Verdict::Unknown;
    };
    let Some(kind) = path_kind(&path, env) else {
        return Verdict::Unknown;
    };
    table_eq_verdict(&kind, &table)
}

/// The **flow-narrowed** kind in force for a guard subject, or `None` when the
/// subject was not tightened by an active flow narrowing in this scope.
///
/// This is the gate that keeps dead-branch folding to *conditional-flow*
/// deadness: a verdict is drawn **only** from a kind a prior guard actually
/// narrowed — a bare param whose binding was flow-tightened
/// ([`StatementEnv::is_param_narrowed`]) or a field path with a narrowing
/// override ([`StatementEnv::narrowed_path`]). A subject sitting at its base
/// declared/seeded binding returns `None` → `Unknown` → the branch is kept, so
/// an idiomatic defensive `IF $p = NONE` on a declared-non-optional `$p` is
/// never greyed. The schema-stepped *declared* kind is deliberately **not** a
/// fallback here — that would fold on the base binding.
fn path_kind(path: &GuardPath, env: &StatementEnv) -> Option<Kind> {
    if path.is_bare() {
        return env
            .is_param_narrowed(&path.param)
            .then(|| env.let_fact(&path.param).and_then(|fact| fact.kind.clone()))
            .flatten();
    }
    env.narrowed_path(&path.key()).cloned()
}

/// The verdict of `$x = <sentinel>` (NONE when `is_none`, else NULL) against
/// `kind`. `AlwaysFalse` when the value can never be that sentinel;
/// `AlwaysTrue` when it can be *nothing but* that sentinel; else `Unknown`.
/// `Kind::Any` can be anything, so it is `Unknown` in both directions.
fn sentinel_eq_verdict(kind: &Kind, is_none: bool) -> Verdict {
    let (can_be, has_other) = if is_none {
        (kind_can_be_none(kind), kind_has_non_none(kind))
    } else {
        (kind_can_be_null(kind), kind_has_non_null(kind))
    };
    if !can_be {
        // The value is never the sentinel → the equality never holds.
        Verdict::AlwaysFalse
    } else if !has_other {
        // The value is *only* the sentinel → the equality always holds.
        Verdict::AlwaysTrue
    } else {
        Verdict::Unknown
    }
}

/// The verdict of `type::table($x) = 'table'` against `kind`. `AlwaysFalse`
/// when the record union provably excludes `table`; `AlwaysTrue` only when the
/// union is *exactly* `{table}`. Any kind that is not a pinned-down record
/// union (an open `record<>`, an `Any`, a union with a non-record variant) is
/// `Unknown` — [`record_tables`] bails, so no branch is greyed on an open union.
fn table_eq_verdict(kind: &Kind, table: &str) -> Verdict {
    let Some(tables) = record_tables(kind) else {
        return Verdict::Unknown;
    };
    let present = tables.iter().any(|t| t == table);
    if !present {
        Verdict::AlwaysFalse
    } else if tables.iter().all(|t| t == table) {
        // The union is exactly `{table}` — the discriminant can be nothing else.
        Verdict::AlwaysTrue
    } else {
        Verdict::Unknown
    }
}

/// Whether some value of `kind` can be `NONE`. `Any` can, so it counts.
fn kind_can_be_none(kind: &Kind) -> bool {
    match kind {
        Kind::None | Kind::Any => true,
        Kind::Either(variants) => variants.iter().any(kind_can_be_none),
        _ => false,
    }
}

/// Whether some value of `kind` is *not* `NONE` (a `NULL` counts: `NULL = NONE`
/// is FALSE). `Any` can be a non-none value, so it counts.
fn kind_has_non_none(kind: &Kind) -> bool {
    match kind {
        Kind::None => false,
        Kind::Either(variants) => variants.iter().any(kind_has_non_none),
        _ => true,
    }
}

/// Whether some value of `kind` can be `NULL`. `Any` can, so it counts.
fn kind_can_be_null(kind: &Kind) -> bool {
    match kind {
        Kind::Null | Kind::Any => true,
        Kind::Either(variants) => variants.iter().any(kind_can_be_null),
        _ => false,
    }
}

/// Whether some value of `kind` is *not* `NULL` (a `NONE` counts: `NONE = NULL`
/// is FALSE). `Any` can be a non-null value, so it counts.
fn kind_has_non_null(kind: &Kind) -> bool {
    match kind {
        Kind::Null => false,
        Kind::Either(variants) => variants.iter().any(kind_has_non_null),
        _ => true,
    }
}

/// The concrete set of record tables `kind` can be, only when it is a **pinned
/// record union** — a `record<a>` or `record<a | b>`, or an `Either` of such,
/// with **no** other possibility (no `none`/`null`, no `Any`, no open
/// `record<>`, no scalar variant). Any looseness returns `None`, so a verdict
/// is only ever drawn from a fully-pinned record union.
fn record_tables(kind: &Kind) -> Option<Vec<String>> {
    match kind {
        Kind::Record(tables) if !tables.is_empty() => {
            Some(tables.iter().map(Table::to_string).collect())
        }
        Kind::Either(variants) => {
            let mut all = Vec::new();
            for variant in variants {
                all.extend(record_tables(variant)?);
            }
            (!all.is_empty()).then_some(all)
        }
        _ => None,
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
pub(crate) fn branch_reachability_in_env(stmt: &ast::IfElseStmt, env: &StatementEnv) -> Reachability {
    let mut branches = Vec::with_capacity(stmt.branches.len());
    // Set once an earlier branch is proven always-taken.
    let mut taken = false;
    for branch in &stmt.branches {
        if taken {
            branches.push(BranchReach::DeadAfterTrue);
            continue;
        }
        match guard_verdict(&branch.condition.node, env) {
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
    use surrealdb_types::KindLiteral;
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

    // The tests below are about the hand-written recognizers *themselves* —
    // which shapes they match and which [`Narrowing`] each yields — so they
    // read the recognizer path whatever the gate says. The fact layer's own
    // answers are asserted in `facts::guard` and `facts::refine`, and the two
    // paths are compared over the whole corpus in `tests/fact_layer.rs`.
    // These three shadow the module's functions for the whole test module.

    fn positive_effects(cond: &ast::Expr, env: &StatementEnv) -> Vec<Effect> {
        with_fact_layer(false, || super::positive_effects(cond, env))
    }

    fn negative_effects(cond: &ast::Expr, env: &StatementEnv) -> Vec<Effect> {
        with_fact_layer(false, || super::negative_effects(cond, env))
    }

    fn where_effects(cond: &ast::Expr) -> Vec<RowEffect> {
        with_fact_layer(false, || super::where_effects(cond))
    }

    /// The WHERE condition of a SELECT, lowered exactly as the result-type
    /// post-pass consumes it.
    fn where_cond(query: &str) -> ast::Expr {
        let parsed = parse_source(SourceId::new("narrow:test"), query).expect("parses");
        match surrealguard_syntax::lower::lower_first_statement(&parsed, "SelectStatement")
            .expect("select statement exists")
            .node
        {
            ast::Statement::Select(stmt) => stmt.where_clause.expect("has a WHERE clause").node,
            other => panic!("expected select, got {other:?}"),
        }
    }

    // --- WHERE-narrowing: leaf-shape recognition (design §3.1) -------------

    #[test]
    fn where_not_none_strips_none_only() {
        // `!= NONE` / `IS NOT NONE` keep `null` — a NULL row survives the filter.
        for query in [
            "SELECT * FROM user WHERE email != NONE;",
            "SELECT * FROM user WHERE email IS NOT NONE;",
        ] {
            assert_eq!(
                where_effects(&where_cond(query)),
                vec![RowEffect {
                    fields: vec!["email".into()],
                    narrowing: Narrowing::StripNone,
                }],
                "{query}"
            );
        }
    }

    #[test]
    fn where_not_null_strips_null_only() {
        assert_eq!(
            where_effects(&where_cond("SELECT * FROM user WHERE note != NULL;")),
            vec![RowEffect {
                fields: vec!["note".into()],
                narrowing: Narrowing::NotNull,
            }]
        );
    }

    #[test]
    fn where_eq_none_is_a_noop() {
        // `f = NONE` is absent from the recognized table — no tightening.
        assert!(where_effects(&where_cond("SELECT * FROM user WHERE email = NONE;")).is_empty());
    }

    #[test]
    fn where_literal_eq_pins_the_field() {
        assert_eq!(
            where_effects(&where_cond("SELECT * FROM user WHERE status = 'active';")),
            vec![RowEffect {
                fields: vec!["status".into()],
                narrowing: Narrowing::Eq(Kind::Literal(KindLiteral::String("active".into()))),
            }]
        );
        // Reversed operand order recognizes the same effect.
        assert_eq!(
            where_effects(&where_cond("SELECT * FROM user WHERE 'active' = status;")),
            vec![RowEffect {
                fields: vec!["status".into()],
                narrowing: Narrowing::Eq(Kind::Literal(KindLiteral::String("active".into()))),
            }]
        );
    }

    #[test]
    fn where_greater_than_strips_none_and_null() {
        for query in [
            "SELECT * FROM user WHERE age > 18;",
            "SELECT * FROM user WHERE age >= 18;",
            // `18 < age` is `age > 18` — the field is on the high side.
            "SELECT * FROM user WHERE 18 < age;",
        ] {
            assert_eq!(
                where_effects(&where_cond(query)),
                vec![RowEffect {
                    fields: vec!["age".into()],
                    narrowing: Narrowing::NotNone,
                }],
                "{query}"
            );
        }
    }

    #[test]
    fn where_less_than_narrows_nothing() {
        // The classic soundness trap: `NONE < 65` is TRUE, so NONE rows survive.
        for query in [
            "SELECT * FROM user WHERE age < 65;",
            "SELECT * FROM user WHERE age <= 65;",
            // `65 > age` is `age < 65` — still the low side.
            "SELECT * FROM user WHERE 65 > age;",
        ] {
            assert!(
                where_effects(&where_cond(query)).is_empty(),
                "{query} must narrow nothing"
            );
        }
    }

    #[test]
    fn where_type_table_narrows_the_record_union() {
        assert_eq!(
            where_effects(&where_cond(
                "SELECT * FROM file WHERE type::table(owner) = 'user';"
            )),
            vec![RowEffect {
                fields: vec!["owner".into()],
                narrowing: Narrowing::Table("user".into()),
            }]
        );
        assert_eq!(
            where_effects(&where_cond(
                "SELECT * FROM file WHERE type::table(owner) != 'user';"
            )),
            vec![RowEffect {
                fields: vec!["owner".into()],
                narrowing: Narrowing::NotTable("user".into()),
            }]
        );
    }

    #[test]
    fn where_and_unions_both_sides() {
        assert_eq!(
            where_effects(&where_cond(
                "SELECT * FROM file WHERE email != NONE AND type::table(owner) = 'user';"
            )),
            vec![
                RowEffect {
                    fields: vec!["email".into()],
                    narrowing: Narrowing::StripNone,
                },
                RowEffect {
                    fields: vec!["owner".into()],
                    narrowing: Narrowing::Table("user".into()),
                },
            ]
        );
    }

    #[test]
    fn where_or_narrows_nothing() {
        assert!(where_effects(&where_cond(
            "SELECT * FROM user WHERE role = 'admin' OR role = 'mod';"
        ))
        .is_empty());
    }

    // --- WHERE-narrowing: narrow_kind soundness on the new arms ------------

    #[test]
    fn strip_none_keeps_null() {
        // `email: string | none | null` → `string | null` (KEEP null).
        let kind = Kind::Either(vec![Kind::None, Kind::Null, Kind::String]);
        assert_eq!(
            narrow_kind(&kind, &Narrowing::StripNone),
            Some(Kind::Either(vec![Kind::Null, Kind::String]))
        );
        // `option<string>` collapses to `string`.
        assert_eq!(
            narrow_kind(&Kind::Either(vec![Kind::None, Kind::String]), &Narrowing::StripNone),
            Some(Kind::String)
        );
        // No `none` present: unchanged.
        assert_eq!(narrow_kind(&Kind::String, &Narrowing::StripNone), None);
    }

    #[test]
    fn not_null_keeps_none() {
        let kind = Kind::Either(vec![Kind::None, Kind::Null, Kind::String]);
        assert_eq!(
            narrow_kind(&kind, &Narrowing::NotNull),
            Some(Kind::Either(vec![Kind::None, Kind::String]))
        );
    }

    #[test]
    fn eq_narrow_pins_only_when_the_base_unifies() {
        let active = Kind::Literal(KindLiteral::String("active".into()));
        // Base unifies (`string` = `string`): pin to the literal.
        assert_eq!(
            narrow_kind(&Kind::String, &Narrowing::Eq(active.clone())),
            Some(active.clone())
        );
        // Through an option: `= lit` also drops the none.
        assert_eq!(
            narrow_kind(
                &Kind::Either(vec![Kind::None, Kind::String]),
                &Narrowing::Eq(active.clone())
            ),
            Some(active.clone())
        );
        // Base disjoint (`int` vs a string literal): cannot apply.
        assert_eq!(narrow_kind(&Kind::Int, &Narrowing::Eq(active)), None);
    }

    #[test]
    fn type_is_record_with_table_narrows_positive_branch_only() {
        // `type::is_record($auth, 'user')` narrows `$auth` to `record<user>` in
        // the THEN branch; the ELSE branch narrows nothing — false is satisfied
        // by NONE, a non-record scalar, or another table.
        let env = StatementEnv::default();
        let c = call_cond("RETURN type::is_record($auth, 'user');");
        assert_eq!(
            positive_effects(&c, &env),
            vec![Effect {
                path: GuardPath::bare("auth".into()),
                narrowing: Narrowing::Table("user".into()),
            }]
        );
        assert!(
            negative_effects(&c, &env).is_empty(),
            "the negative branch of type::is_record narrows nothing"
        );
    }

    #[test]
    fn bare_type_is_record_strips_none_on_positive_branch_only() {
        // `type::is_record($auth)` (no table arg) strips the NONE, leaving the
        // non-none open `record`; the negative branch narrows nothing.
        let env = StatementEnv::default();
        let c = call_cond("RETURN type::is_record($auth);");
        assert_eq!(
            positive_effects(&c, &env),
            vec![Effect {
                path: GuardPath::bare("auth".into()),
                narrowing: Narrowing::NotNone,
            }]
        );
        assert!(negative_effects(&c, &env).is_empty());
    }

    #[test]
    fn non_is_record_call_narrows_nothing() {
        // A different call is not a recognized guard.
        let env = StatementEnv::default();
        let c = call_cond("RETURN type::is_string($auth);");
        assert!(positive_effects(&c, &env).is_empty());
        assert!(negative_effects(&c, &env).is_empty());
    }

    #[test]
    fn none_guards_narrow_both_polarities() {
        // Each sentinel guard eliminates ONLY its own sentinel. `NULL = NONE`
        // is FALSE on the engine, so a NULL value passes a `= NONE` guard and
        // reaches the code after it: `!= NONE` is `StripNone` (keep `null`),
        // never the both-sentinel `NotNone`. `!= NULL` is the mirror.
        let env = StatementEnv::default();
        for (query, pos, neg) in [
            ("RETURN $x = NONE;", Narrowing::None, Narrowing::StripNone),
            ("RETURN $x != NONE;", Narrowing::StripNone, Narrowing::None),
            ("RETURN $x IS NONE;", Narrowing::None, Narrowing::StripNone),
            ("RETURN $x IS NOT NONE;", Narrowing::StripNone, Narrowing::None),
            ("RETURN $x = NULL;", Narrowing::Null, Narrowing::NotNull),
            ("RETURN $x != NULL;", Narrowing::NotNull, Narrowing::Null),
            ("RETURN $x IS NULL;", Narrowing::Null, Narrowing::NotNull),
            ("RETURN $x IS NOT NULL;", Narrowing::NotNull, Narrowing::Null),
        ] {
            let c = cond(query);
            assert_eq!(
                positive_effects(&c, &env),
                vec![Effect {
                    path: GuardPath::bare("x".into()),
                    narrowing: pos
                }],
                "positive: {query}"
            );
            assert_eq!(
                negative_effects(&c, &env),
                vec![Effect {
                    path: GuardPath::bare("x".into()),
                    narrowing: neg
                }],
                "negative: {query}"
            );
        }
    }

    #[test]
    fn none_guard_on_a_field_path_narrows_that_exact_path() {
        // `$file.folder != NONE` narrows the path `file.folder`, not `$file`.
        let env = StatementEnv::default();
        let path = GuardPath {
            param: "file".into(),
            fields: vec!["folder".into()],
        };
        let c = cond("RETURN $file.folder != NONE;");
        assert_eq!(
            positive_effects(&c, &env),
            vec![Effect {
                path: path.clone(),
                narrowing: Narrowing::StripNone
            }]
        );
        assert_eq!(
            negative_effects(&c, &env),
            vec![Effect {
                path,
                narrowing: Narrowing::None
            }]
        );
    }

    #[test]
    fn type_table_discriminant_narrows_records() {
        let env = StatementEnv::default();
        let c = cond("RETURN type::table($r) = 'folder';");
        assert_eq!(
            positive_effects(&c, &env),
            vec![Effect {
                path: GuardPath::bare("r".into()),
                narrowing: Narrowing::Table("folder".into())
            }]
        );
        assert_eq!(
            negative_effects(&c, &env),
            vec![Effect {
                path: GuardPath::bare("r".into()),
                narrowing: Narrowing::NotTable("folder".into())
            }]
        );
    }

    #[test]
    fn in_guard_narrows_subject_to_the_collection_element_kind() {
        // `$subject IN $a` where `$a : array<string>` narrows `$subject` to
        // `string` on the POSITIVE branch (occurrence typing); the negative
        // branch narrows nothing — "not in" proves nothing about the value.
        let env = env_base("a", Kind::Array(Box::new(Kind::String), None));
        let c = cond("RETURN $subject IN $a;");
        assert_eq!(
            positive_effects(&c, &env),
            vec![Effect {
                path: GuardPath::bare("subject".into()),
                narrowing: Narrowing::Is(Kind::String),
            }]
        );
        assert!(
            negative_effects(&c, &env).is_empty(),
            "not-in narrows nothing"
        );

        // A `set<T>` collection narrows the same way.
        let set_env = env_base("a", Kind::Set(Box::new(Kind::Int), None));
        assert_eq!(
            positive_effects(&c, &set_env),
            vec![Effect {
                path: GuardPath::bare("subject".into()),
                narrowing: Narrowing::Is(Kind::Int),
            }]
        );

        // A bare `array` (element `any`) is undeterminable → no effect. Likewise
        // an unbound / non-collection collection operand.
        let any_env = env_base("a", Kind::Array(Box::new(Kind::Any), None));
        assert!(positive_effects(&c, &any_env).is_empty());
        assert!(positive_effects(&c, &StatementEnv::default()).is_empty());
    }

    #[test]
    fn indirect_discriminant_resolves_a_let_bound_type_table() {
        // `LET $t = type::table($r); IF $t = 'folder'` narrows `$r` the same
        // as the direct call form.
        let mut env = StatementEnv::default();
        env.set_table_discriminant("t".into(), Some("r".into()));
        let c = cond("RETURN $t = 'folder';");
        assert_eq!(
            positive_effects(&c, &env),
            vec![Effect {
                path: GuardPath::bare("r".into()),
                narrowing: Narrowing::Table("folder".into())
            }]
        );
        // Without the mapping, `$t = 'folder'` narrows nothing.
        let bare = StatementEnv::default();
        assert!(positive_effects(&c, &bare).is_empty());
    }

    #[test]
    fn or_of_none_guards_yields_both_negatives_on_fall_through() {
        // `$a = NONE OR $b = NONE` proves nothing when true, but its
        // fall-through proves both are non-none.
        let env = StatementEnv::default();
        let c = cond("RETURN $a = NONE OR $b = NONE;");
        assert!(positive_effects(&c, &env).is_empty());
        assert_eq!(
            negative_effects(&c, &env),
            vec![
                Effect {
                    path: GuardPath::bare("a".into()),
                    narrowing: Narrowing::StripNone
                },
                Effect {
                    path: GuardPath::bare("b".into()),
                    narrowing: Narrowing::StripNone
                }
            ]
        );
    }

    #[test]
    fn each_sentinel_refinement_removes_only_its_own_sentinel() {
        // `option<string | null>`. Verified on SurrealDB 3.x:
        //   RETURN NULL = NONE   -> false      RETURN NONE IS NULL     -> false
        //   RETURN NULL != NONE  -> true       RETURN NONE IS NOT NULL -> true
        // so a `!= NONE` guard leaves `null` reachable and a `!= NULL` guard
        // leaves `none` reachable. Collapsing either to the both-sentinel
        // `NotNone` produces a type the database can violate.
        let optional_nullable = Kind::Either(vec![Kind::None, Kind::Null, Kind::String]);

        assert_eq!(
            narrow_kind(&optional_nullable, &Narrowing::StripNone),
            Some(Kind::Either(vec![Kind::Null, Kind::String])),
            "`!= NONE` must keep `null`"
        );
        assert_eq!(
            narrow_kind(&optional_nullable, &Narrowing::NotNull),
            Some(Kind::Either(vec![Kind::None, Kind::String])),
            "`!= NULL` must keep `none`"
        );
        assert_eq!(
            narrow_kind(&optional_nullable, &Narrowing::None),
            Some(Kind::None)
        );
        assert_eq!(
            narrow_kind(&optional_nullable, &Narrowing::Null),
            Some(Kind::Null)
        );

        // A plain `option<string>` has no `null` to keep, so both the sentinel
        // refinement and the both-sentinel one land on `string`.
        let optional = Kind::Either(vec![Kind::None, Kind::String]);
        assert_eq!(
            narrow_kind(&optional, &Narrowing::StripNone),
            Some(Kind::String)
        );
        assert_eq!(
            narrow_kind(&optional, &Narrowing::NotNone),
            Some(Kind::String)
        );

        // The ordering guards keep the both-sentinel refinement: `NONE > 18`
        // and `NULL > 18` are both FALSE, so both are filtered out.
        assert_eq!(
            narrow_kind(&optional_nullable, &Narrowing::NotNone),
            Some(Kind::String)
        );
    }

    #[test]
    fn record_union_narrows_to_and_without_a_table() {
        let union = Kind::Record(vec![Table::from("file"), Table::from("folder")]);
        assert_eq!(
            narrow_kind(&union, &Narrowing::Table("folder".into())),
            Some(Kind::Record(vec![Table::from("folder")]))
        );
        assert_eq!(
            narrow_kind(&union, &Narrowing::NotTable("folder".into())),
            Some(Kind::Record(vec![Table::from("file")]))
        );
        // A table not in the union cannot be narrowed to.
        assert_eq!(narrow_kind(&union, &Narrowing::Table("ghost".into())), None);
    }

    #[test]
    fn not_none_strips_the_option() {
        let option = Kind::Either(vec![Kind::None, Kind::String]);
        assert_eq!(
            narrow_kind(&option, &Narrowing::NotNone),
            Some(Kind::String)
        );
        // A non-optional kind is left unchanged (no effect).
        assert_eq!(narrow_kind(&Kind::String, &Narrowing::NotNone), None);
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
        assert_eq!(code_count(&guarded, 5002), 0, "AND-right sees the narrowed path");

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
        assert_eq!(code_count(&guarded, 5002), 0, "THEN body sees the narrowed path");
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
        assert_eq!(code_count(&unguarded, 5002), 1, "option<record> is not record");
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
        assert_eq!(code_count(&guarded, 5002), 0, "!= NONE narrows $auth to record");
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
        assert_eq!(code_count(&guarded, 5002), 0, "positive branch narrows to record<user>");

        // The same call without the guard genuinely fails — the guard is what
        // clears it (also proves the double-colon spelling normalizes).
        let colon = format!(
            "{AUTH_TABLES}\
             DEFINE FUNCTION fn::caller() {{\n\
                IF type::is::record($auth, 'user') THEN RETURN fn::user_only($auth) END;\n\
                RETURN false;\n\
             }};"
        );
        assert_eq!(code_count(&colon, 5002), 0, "type::is::record normalizes to type::is_record");
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
        assert_eq!(code_count(&guarded, 5002), 0, "IN narrows $s to int in the arg");

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
        assert_eq!(code_count(&guarded, 5002), 0, "THEN body sees the narrowed $s");
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
        assert_eq!(code_count(&guarded, 5002), 0, "both $x and $y narrowed for the tail call");

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
        assert_eq!(code_count(&leaky, 5002), 1, "narrowing must not leak past the AND");
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

    /// The verdict of the guard in `IF <guard> { ... }`, against `env`.
    fn verdict(guard: &str, env: &StatementEnv) -> Verdict {
        let query = format!("RETURN {guard};");
        // A bare boolean call guard (`type::is_record(...)`) lowers as a call;
        // every other guard here is a comparison (binary) expression.
        let expr = if guard.starts_with("type::is_record") {
            call_cond(&query)
        } else {
            cond(&query)
        };
        guard_verdict(&expr, env)
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
        assert_eq!(verdict("$x = NONE", &StatementEnv::default()), Verdict::Unknown);
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
        assert_eq!(verdict("type::table($x) = 'a'", &rec_b), Verdict::AlwaysFalse);
        assert_eq!(verdict("type::table($x) != 'a'", &rec_b), Verdict::AlwaysTrue);
        // Exactly `{a}` → always true; `!= 'a'` → always false.
        let rec_a = env_with("x", record("a"));
        assert_eq!(verdict("type::table($x) = 'a'", &rec_a), Verdict::AlwaysTrue);
        assert_eq!(verdict("type::table($x) != 'a'", &rec_a), Verdict::AlwaysFalse);
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
        assert_eq!(verdict("type::is_record($x, 'a')", &rec_b), Verdict::AlwaysFalse);
        let rec_a = env_with("x", record("a"));
        assert_eq!(verdict("type::is_record($x, 'a')", &rec_a), Verdict::AlwaysTrue);
        // A none-carrying subject is not a pinned record union → Unknown (and
        // indeed `is_record` genuinely varies: false on NONE, true on record<a>).
        let opt_a = env_with("x", Kind::Either(vec![Kind::None, record("a")]));
        assert_eq!(verdict("type::is_record($x, 'a')", &opt_a), Verdict::Unknown);
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
