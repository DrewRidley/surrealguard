//! Expression fact inference over the typed AST.
//!
//! Pure fact inference over `ast::Expr` — no source-text reading for
//! structure, no CST shapes. (`crate::expression` holds the fact types.)
//!
//! [`infer_expression_fact`] computes facts over the one
//! [`AnalysisContext`]; it never emits findings itself (inference never
//! checks), but carrying the context means checking functions invoked
//! along the way — and the statement cores it recurses into — share the
//! same diagnostics sink, environment, and row table.

use surrealdb_types::{Kind, KindLiteral};
use surrealguard_syntax::ast;
use surrealguard_syntax::span::SourceSpan;

use crate::analyzer::context::AnalysisContext;
use crate::expression::{ExpressionFact, ExpressionValueClass, PartialReason};
use crate::schema::SchemaIndex;
use crate::statement_env::StatementEnv;

pub fn infer_expression_fact(
    expr: &ast::Spanned<ast::Expr>,
    ctx: &mut AnalysisContext<'_>,
) -> ExpressionFact {
    let span = SourceSpan::new(ctx.source().clone(), expr.span);
    match &expr.node {
        ast::Expr::Literal(literal) => literal_fact(literal, span),
        ast::Expr::Param(name) => {
            // Host-facing parameter uses are recorded at their own site
            // with their exact span. A name the engine binds itself is never
            // one, whatever scope binds it — that is why `$this`, `$parent`,
            // `$self` and `$scope` were being demanded from the caller and
            // emitted by codegen.
            if ctx.env().let_fact(name).is_none() && !crate::context_params::is_engine_param(name) {
                ctx.record_param_use(name.clone(), span.clone());
            }
            param_fact(name, span, ctx.env())
        }
        ast::Expr::Table(name) => scalar_fact(
            span,
            ExpressionValueClass::Literal,
            Kind::Table(vec![name.node.as_str().into()]),
        ),
        // A record *range* (`person:1..5`) denotes many records of the table,
        // so it is an array of them — a plain id is exactly one.
        ast::Expr::RecordId { table, range, .. } => {
            let record = Kind::Record(vec![table.node.as_str().into()]);
            let kind = if *range {
                Kind::Array(Box::new(record), None)
            } else {
                record
            };
            scalar_fact(span, ExpressionValueClass::Literal, kind)
        }
        ast::Expr::Constant(path) => constant_fact(path, span),
        ast::Expr::Range(range) => range_fact(range, span, ctx),
        ast::Expr::Idiom(idiom) => idiom_fact(idiom, span, ctx),
        ast::Expr::Binary { lhs, op, rhs } => binary_fact(lhs, op, rhs, span, ctx),
        ast::Expr::Prefix { op, expr } => prefix_fact(op, expr, span, ctx),
        ast::Expr::Object(fields) => object_fact(fields, span, ctx),
        ast::Expr::Array(elements) => array_fact(elements, span, ctx),
        ast::Expr::Call(call) => call_fact(call, span, ctx),
        ast::Expr::Cast { ty, expr } => cast_fact(ty, expr, span, ctx),
        ast::Expr::Subquery(inner) => {
            let fact = ExpressionFact::new(span, ExpressionValueClass::Subquery);
            match statement_value_kind(inner, ctx) {
                Some(kind) => fact.with_kind(kind),
                None => fact.with_partial(PartialReason::UnsupportedSyntax("SubQuery".into())),
            }
        }
        // Blocks thread an environment through their statements, which needs
        // the ctx path (`analyze_expr`); pure inference reports them as
        // partial rather than guessing.
        ast::Expr::Block(_) => partial_fact(span, ExpressionValueClass::Block, "Block".into()),
        ast::Expr::Closure(closure) => closure_fact(closure, span, ctx),
        ast::Expr::Partial(partial) => partial_fact(
            span,
            ExpressionValueClass::Unknown,
            partial.cst_kind.clone(),
        ),
    }
}

/// The response kind of a statement used as a value (subqueries,
/// `FROM (SELECT ...)` sources). Covers the statement kinds whose response
/// typing is pure; environment-threading statements need the ctx path.
pub fn statement_value_kind(
    stmt: &ast::Spanned<ast::Statement>,
    ctx: &mut AnalysisContext<'_>,
) -> Option<Kind> {
    let kind = match &stmt.node {
        ast::Statement::Select(s) => crate::analyzer::data::select::select_response_kind(s, ctx),
        ast::Statement::Create(s) => crate::analyzer::data::create::create_response_kind(s, ctx),
        ast::Statement::Update(s) => crate::analyzer::data::update::update_response_kind(s, ctx),
        ast::Statement::Upsert(s) => crate::analyzer::data::upsert::upsert_response_kind(s, ctx),
        ast::Statement::Delete(s) => crate::analyzer::data::delete::delete_response_kind(s, ctx),
        ast::Statement::Insert(s) => crate::analyzer::data::insert::insert_response_kind(s, ctx),
        ast::Statement::Relate(s) => crate::analyzer::data::relate::relate_response_kind(s, ctx),
        ast::Statement::Return(s) => match &s.value {
            Some(value) => infer_expression_fact(value, ctx).kind.unwrap_or(Kind::Any),
            None => Kind::None,
        },
        ast::Statement::Expr(e) => infer_expression_fact(e, ctx).kind.unwrap_or(Kind::Any),
        // An IF used as a value (`RETURN IF c { a } ELSE { b }`, `LET $y = IF
        // c { ... }`, which lower to a `Subquery`): route it through the same
        // full flow analysis a statement `IF` uses, so every reachable branch
        // body is checked with the complete rule set (unknown table/field,
        // type mismatches, arg-kind, index-required, …) — not just the
        // inference-side subset `pure_if_else_kind` happened to trigger. The
        // analysis is scoped in a child env so any branch-guard narrowing is
        // discarded (an IF-expression has no fall-through into the surrounding
        // scope), and only reachable branches run (dead arms are greyed, never
        // checked). Re-inference of the same expression re-emits identical
        // findings, which `ctx.emit` dedupes — so this stays single-emit even
        // when the subquery sits inside a binary/call that re-reads its kind.
        ast::Statement::IfElse(s) => ctx.with_child_env(|ctx| {
            crate::analyzer::flow::if_else::analyze_if_else_flow(ctx, s).into_kind()
        }),
        // `({ … })` — a block used as a value. Routed through the same full
        // flow analysis a statement block gets, for the same reason the `IF`
        // arm above is: pure inference walks the block's statements and checks
        // none of their contracts, so `RETURN ({ RETURN 1 + 'a'; })` was silent
        // while the identical unparenthesized block reported. The child env
        // keeps the block's `LET`s from leaking, which is the boundary a block
        // *is*.
        ast::Statement::Block(s) => {
            ctx.with_child_env(|ctx| crate::analyzer::flow::block::analyze_block(ctx, s))
        }
        _ => return None,
    };
    Some(kind)
}

/// A closure value's own type: `Kind::Function(params, return)`. The
/// return kind comes from the declaration, or from the body inferred with
/// the declared parameter kinds — call sites re-infer with their actual
/// argument kinds ([`closure_return_kind`]).
fn closure_fact(
    closure: &ast::Closure,
    span: SourceSpan,
    ctx: &mut AnalysisContext<'_>,
) -> ExpressionFact {
    let param_kinds = closure_param_kinds(closure, ctx);
    let return_kind = closure_return_kind(closure, &param_kinds, ctx);

    ExpressionFact::new(span, ExpressionValueClass::Literal)
        .with_kind(Kind::Function(Some(param_kinds), return_kind.map(Box::new)))
}

fn declared_kind(
    ty: Option<&ast::Spanned<ast::TypeExpr>>,
    ctx: &mut AnalysisContext<'_>,
) -> Option<Kind> {
    crate::schema::kind_from_type_expr(&ty?.node, ctx.source_text()).kind
}

/// The closure's return kind when invoked with `arg_kinds` — the declared
/// return type when present, otherwise the body inferred with each
/// parameter bound to its argument kind (falling back to the declared
/// parameter type, then `Any`).
pub fn closure_return_kind(
    closure: &ast::Closure,
    arg_kinds: &[Kind],
    ctx: &mut AnalysisContext<'_>,
) -> Option<Kind> {
    if let Some(declared) = declared_kind(closure.return_ty.as_ref(), ctx) {
        return Some(declared);
    }

    ctx.with_child_env(|ctx| {
        bind_closure_params(closure, arg_kinds, ctx);
        match &closure.body.node {
            ast::Expr::Block(block) => pure_block_kind(block, ctx),
            _ => infer_expression_fact(&closure.body, ctx).kind,
        }
    })
}

/// The closure's parameters at their **declared** kinds, `any` where a
/// parameter carries no type. What is known about a closure read on its own,
/// before any call site says what it is applied to.
pub(crate) fn closure_param_kinds(
    closure: &ast::Closure,
    ctx: &mut AnalysisContext<'_>,
) -> Vec<Kind> {
    closure
        .params
        .iter()
        .map(|(_, ty)| declared_kind(ty.as_ref(), ctx).unwrap_or(Kind::Any))
        .collect()
}

/// Binds a closure's parameters in the **current** scope — which must already
/// be the closure's own child scope, since these bindings are exactly what
/// must not escape it.
///
/// `arg_kinds` are the kinds the closure is applied to; a position without one
/// (or one carrying no information) falls back to the declared type, then to
/// `any`.
pub(crate) fn bind_closure_params(
    closure: &ast::Closure,
    arg_kinds: &[Kind],
    ctx: &mut AnalysisContext<'_>,
) {
    for (index, (name, ty)) in closure.params.iter().enumerate() {
        let kind = match arg_kinds.get(index) {
            Some(kind) if *kind != Kind::Any => kind.clone(),
            _ => declared_kind(ty.as_ref(), ctx).unwrap_or(Kind::Any),
        };
        let mut fact = ExpressionFact::new(
            SourceSpan::new(ctx.source().clone(), name.span),
            ExpressionValueClass::Variable,
        );
        fact.kind = Some(kind);
        ctx.define_local(name.node.clone(), fact);
    }
}

/// The value of a block in pure inference: threads `LET` bindings through
/// a child scope and returns on `RETURN` or the final statement's value.
/// Environment-mutating statements beyond `LET` (IF/FOR with effects) are
/// out of pure reach.
fn pure_block_kind(block: &ast::Block, ctx: &mut AnalysisContext<'_>) -> Option<Kind> {
    ctx.with_child_env(|ctx| {
        let mut last = Kind::None;
        for statement in &block.statements {
            match &statement.node {
                ast::Statement::Let(stmt) => {
                    let fact = infer_expression_fact(&stmt.value, ctx);
                    ctx.define_local(stmt.name.node.clone(), fact);
                }
                ast::Statement::Return(stmt) => {
                    return match &stmt.value {
                        Some(value) => infer_expression_fact(value, ctx).kind,
                        None => Some(Kind::None),
                    };
                }
                _ => last = statement_value_kind(statement, ctx)?,
            }
        }
        Some(last)
    })
}

fn literal_fact(literal: &ast::Literal, span: SourceSpan) -> ExpressionFact {
    let kind = match literal {
        ast::Literal::Int(_) => Kind::Int,
        ast::Literal::Float(_) => Kind::Float,
        ast::Literal::Decimal => Kind::Decimal,
        ast::Literal::String(_) => Kind::String,
        ast::Literal::Bool(_) => Kind::Bool,
        ast::Literal::None => Kind::None,
        ast::Literal::Null => Kind::Null,
        ast::Literal::Duration(_) => Kind::Duration,
        ast::Literal::Datetime(_) => Kind::Datetime,
        ast::Literal::Uuid(_) => Kind::Uuid,
        ast::Literal::Regex(_) => Kind::Regex,
        ast::Literal::Bytes(_) => Kind::Bytes,
        // A file literal names a bucket; the kind lists the buckets a value
        // may belong to, and a literal may belong to any.
        ast::Literal::File(_) => Kind::File(Vec::new()),
    };
    let mut fact = scalar_fact(span, ExpressionValueClass::Literal, kind);
    fact.value = const_literal_value(literal);
    fact
}

/// The kind of a module constant (`math::pi`, `time::epoch`), or `None` for a
/// path the engine has no constant for.
pub(crate) fn constant_kind(path: &str) -> Option<Kind> {
    Some(match path {
        "math::e"
        | "math::frac_1_pi"
        | "math::frac_1_sqrt_2"
        | "math::frac_2_pi"
        | "math::frac_2_sqrt_pi"
        | "math::frac_pi_2"
        | "math::frac_pi_3"
        | "math::frac_pi_4"
        | "math::frac_pi_6"
        | "math::frac_pi_8"
        | "math::inf"
        | "math::infinity"
        | "math::ln_10"
        | "math::ln_2"
        | "math::log10_2"
        | "math::log10_e"
        | "math::log2_10"
        | "math::log2_e"
        | "math::neg_inf"
        | "math::neg_infinity"
        | "math::pi"
        | "math::sqrt_2"
        | "math::tau" => Kind::Float,
        "time::epoch" | "time::minimum" | "time::maximum" => Kind::Datetime,
        "duration::max" => Kind::Duration,
        _ => return None,
    })
}

fn constant_fact(path: &ast::Spanned<String>, span: SourceSpan) -> ExpressionFact {
    match constant_kind(&path.node) {
        Some(kind) => scalar_fact(span, ExpressionValueClass::Literal, kind),
        // Not a constant the engine defines: the value is unknown, and saying
        // so is the honest answer (the engine rejects it at runtime).
        None => partial_fact(
            span,
            ExpressionValueClass::Unknown,
            format!("Constant {}", path.node),
        ),
    }
}

/// `a..b`: always a `range`; the bounds are read for what they depend on.
fn range_fact(
    range: &ast::Range,
    span: SourceSpan,
    ctx: &mut AnalysisContext<'_>,
) -> ExpressionFact {
    let mut fact = ExpressionFact::new(span, ExpressionValueClass::Unknown);
    for bound in [&range.start, &range.end].into_iter().flatten() {
        let inner = infer_expression_fact(bound, ctx);
        merge_dependencies(&mut fact, inner.dependencies);
    }
    fact.with_kind(Kind::Range)
}

/// The literal's static value, for the variants whose value the AST
/// retains. (Duration/datetime/uuid literals keep only their kind.)
fn const_literal_value(literal: &ast::Literal) -> Option<surrealdb_types::Value> {
    use std::str::FromStr;
    use surrealdb_types::Value;
    let value = match literal {
        ast::Literal::String(value) => Value::String(value.clone()),
        ast::Literal::Int(value) => Value::Number(surrealdb_types::Number::Int(*value)),
        ast::Literal::Float(value) => Value::Number(surrealdb_types::Number::Float(*value)),
        ast::Literal::Bool(value) => Value::Bool(*value),
        ast::Literal::None => Value::None,
        ast::Literal::Null => Value::Null,
        ast::Literal::Datetime(text) => {
            Value::Datetime(surrealdb_types::Datetime::from_str(text).ok()?)
        }
        ast::Literal::Duration(text) => {
            Value::Duration(surrealdb_types::Duration::from_str(text).ok()?)
        }
        ast::Literal::Uuid(text) => Value::Uuid(surrealdb_types::Uuid::from_str(text).ok()?),
        _ => return None,
    };
    Some(value)
}

fn param_fact(name: &str, span: SourceSpan, env: &StatementEnv) -> ExpressionFact {
    if let Some(bound) = env.let_fact(name) {
        let mut fact = ExpressionFact::new(span, ExpressionValueClass::Variable);
        fact.kind = bound.kind.clone();
        fact.value = bound.value.clone();
        return fact;
    }

    // A `DEFINE PARAM $x VALUE …` gives the database-side value its kind, and
    // that kind is the contract a host override must also satisfy — so a read
    // of `$x` is not unresolved, it is that kind.
    if let Some(kind) = ctx_param_default_kind(env, name) {
        let mut fact = ExpressionFact::new(span, ExpressionValueClass::Variable).with_kind(kind);
        fact.dependencies.params.push(name.to_string());
        return fact;
    }

    let mut fact = ExpressionFact::new(span, ExpressionValueClass::Variable)
        .with_partial(PartialReason::Unresolved);
    fact.dependencies.params.push(name.to_string());
    fact
}

/// The declared kind of a `DEFINE PARAM` default, when one covers `name`.
fn ctx_param_default_kind(env: &StatementEnv, name: &str) -> Option<Kind> {
    env.param_default_fact(name)?.kind.clone()
}

// ---------------------------------------------------------------------------
// Idioms
// ---------------------------------------------------------------------------

fn idiom_fact(
    idiom: &ast::Idiom,
    span: SourceSpan,
    ctx: &mut AnalysisContext<'_>,
) -> ExpressionFact {
    let mut fact = ExpressionFact::new(span, ExpressionValueClass::FieldPath);

    if let Some(segments) = plain_field_segments(idiom) {
        fact.dependencies.field_paths.push(segments.join("."));
    }

    // Graph traversals are rooted in the row context and resolve through
    // the schema's relations.
    if matches!(
        idiom.parts.first().map(|p| &p.node),
        Some(ast::IdiomPart::Graph { .. })
    ) {
        let Some(table) = ctx.row_table() else {
            return fact.with_partial(PartialReason::Unresolved);
        };
        return match crate::analyzer::data::select::graph_projection_kind(
            &table.name,
            idiom,
            ctx.schema(),
            false,
        ) {
            Some(kind) => fact.with_kind(kind),
            None => fact.with_partial(PartialReason::Unresolved),
        };
    }

    match step_idiom_kind(idiom, ctx) {
        Some(kind) => fact.with_kind(kind),
        None => fact.with_partial(PartialReason::Unresolved),
    }
}

/// The segments of an idiom made purely of `Field` parts, if it is one.
pub fn plain_field_segments(idiom: &ast::Idiom) -> Option<Vec<String>> {
    idiom
        .parts
        .iter()
        .map(|part| match &part.node {
            ast::IdiomPart::Field(name) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// Walks an idiom part by part, tracking the kind of the value so far:
/// row fields, `LET`-bound starts (`$user.name`), record links (stepping
/// through the schema), literal objects, collection indexes, and method
/// calls dispatched by receiver kind.
/// The receiver kind standing before each idiom part, for the
/// checking-side position contracts: `(part, kind-before-part)` pairs.
/// `None` receivers mean the prefix didn't resolve; checking skips them.
///
/// **Every** part is reported, including the first. A leading `Field` and a
/// leading `Graph` both have a receiver — the row — so `john.len()` and
/// `->follows->user` are parts standing on `record<user>` exactly as
/// `author.john` is. Only a leading `Start` has nothing in front of it (it *is*
/// the value), and it is the one part reported with no receiver.
pub fn idiom_prefix_kinds<'i>(
    idiom: &'i ast::Idiom,
    ctx: &mut AnalysisContext<'_>,
) -> Vec<(&'i ast::Spanned<ast::IdiomPart>, Option<Kind>)> {
    let mut result = Vec::new();
    let mut parts = idiom.parts.iter();
    let Some(first) = parts.next() else {
        return result;
    };
    let mut place = start_place(&first.node);
    let mut current: Option<Kind> = match &first.node {
        ast::IdiomPart::Start(expr) => {
            result.push((first, None));
            infer_expression_fact(expr, ctx).kind
        }
        // The row is the receiver of a leading field or traversal step; a
        // leading value is its own receiver-less start.
        _ => {
            let receiver = ctx
                .row_table()
                .map(|table| Kind::Record(vec![table.name.as_str().into()]));
            result.push((first, receiver.clone()));
            receiver.and_then(|kind| step_part_kind(&kind, &first.node, ctx))
        }
    };
    if let Some(narrowed) = narrowed_kind(place.as_ref(), ctx.env()) {
        current = Some(narrowed);
    }
    for part in parts {
        result.push((part, current.clone()));
        let stepped = current.and_then(|kind| step_part_kind(&kind, &part.node, ctx));
        place = place.and_then(|prefix| prefix.stepped(&part.node));
        current = narrowed_kind(place.as_ref(), ctx.env()).or(stepped);
    }
    result
}

/// The place an idiom's **first** part names, when it names one.
///
/// Only a leading value can: a bare leading `Field` is rooted in the row, and
/// the flow environment does not key row fields (a row field named `x` and a
/// `$x` would share a key). The row side has its own oracle
/// (`data::select::ProjectedRow`).
fn start_place(part: &ast::IdiomPart) -> Option<crate::analyzer::facts::Place> {
    match part {
        ast::IdiomPart::Start(expr) => crate::analyzer::facts::place_of(&expr.node),
        _ => None,
    }
}

/// The kind a flow guard proved for the place an idiom prefix names, or `None`
/// when no guard proved one.
///
/// **Longest prefix, not exact key.** A guard narrows a place; a read of a
/// *sub*-path of that place must start from what the guard proved, or the
/// narrowing is present in the environment and unreachable from the read.
/// Walking the prefixes and taking the innermost recorded one does both halves:
/// `IF $x.f != NONE` makes `$x.f.g` resolvable through the narrowed `$x.f`, and
/// makes a contract check on `$x.f.len()` read `string` rather than the
/// `option<string>` the guard ruled out (the F31 false positive).
fn narrowed_kind(
    place: Option<&crate::analyzer::facts::Place>,
    env: &StatementEnv,
) -> Option<Kind> {
    let place = place?;
    // A bare param carries its narrowing in its own binding, not here.
    if place.path.is_empty() {
        return None;
    }
    if !matches!(place.root, crate::analyzer::facts::PlaceRoot::Param(_)) {
        return None;
    }
    env.narrowed_path(&place.key()?).cloned()
}

/// The element kind of a collection, distributed over a union.
///
/// `[[1, 2], [3]]` infers as `array<int, 2> | array<int, 1>` — every arm is
/// a collection, so indexing it is valid SurrealQL and its element kind is
/// the union of the arms' elements. A flat `matches!(k, Array | Set)` sees
/// only the `Either` and gives up, which loses the type *and* (at the
/// checking sites) manufactures a false 2030/5001.
///
/// Non-collection arms are skipped rather than failing the whole lookup:
/// indexing an `option<array<T>>` is a contract violation worth reporting
/// (see [`is_indexable_kind`]), but the reported expression still has the
/// element type of its collection arms, so inference must not collapse.
/// `None` means no arm was a collection at all.
pub(crate) fn collection_element_kind(kind: &Kind) -> Option<Kind> {
    match kind {
        Kind::Array(element, _) | Kind::Set(element, _) => Some((**element).clone()),
        Kind::Either(variants) => {
            let elements: Vec<Kind> = variants
                .iter()
                .filter_map(collection_element_kind)
                .collect();
            (!elements.is_empty()).then(|| Kind::either(elements))
        }
        _ => None,
    }
}

/// Whether index/filter applies to `kind`: a collection, an object,
/// or — distributing over a union — a union whose arms *all* are. One
/// definitely-non-collection arm (the `NONE` of an `option<array<T>>`)
/// makes the access a genuine contract violation.
///
/// `Kind::Any` arms are permissive: an unknown arm proves nothing.
pub(crate) fn is_indexable_kind(kind: &Kind) -> bool {
    let base = crate::kinds::literal_base_kind(kind).unwrap_or_else(|| kind.clone());
    match base {
        Kind::Array(_, _) | Kind::Set(_, _) | Kind::Object | Kind::Any => true,
        Kind::Either(variants) => variants.iter().all(is_indexable_kind),
        _ => false,
    }
}

/// Whether `.*` / `[*]` applies to `kind`.
///
/// The splat accepts everything an index does, plus a **record link**: `.*`
/// on a `record<user>` is not an index at all, it is "give me that row", and
/// the engine returns the user's whole field object (verified on SurrealDB
/// 3.0.5 — `SELECT author.* FROM ONLY post:p1` → `{author: {id, name, …}}`).
/// A scalar still has nothing to splat, and — as with an index — a union with
/// one non-splattable arm (the `NONE` of an `option<record<user>>`) is a
/// genuine contract violation, not a shape to guess through.
pub(crate) fn is_splattable_kind(kind: &Kind) -> bool {
    let base = crate::kinds::literal_base_kind(kind).unwrap_or_else(|| kind.clone());
    match base {
        Kind::Record(_) => true,
        Kind::Either(variants) => variants.iter().all(is_splattable_kind),
        other => is_indexable_kind(&other),
    }
}

/// What `.*` / `[*]` yields.
///
/// A record link becomes the **row it names**: the target table's whole field
/// object, built by the very same `object_kind_for_all_fields` a bare
/// `SELECT *` projects, so a splat and a wildcard SELECT can never disagree.
/// A link naming several tables becomes the union of their rows — the engine
/// hands back whichever row the link points at (`->touched->?.*` on an
/// `OUT user|post` edge returns user rows and post rows in one array,
/// verified on 3.0.5). The layers a link is wrapped in are preserved, so
/// `array<record<user>>` splats to `array<{…}>`. Everything else — a
/// collection, an object — splats to itself.
///
/// `Err(table)` when a named table declares no fields: there is no row shape
/// to expand to, which is exactly where `SELECT * FROM <table>` gives up too.
/// The caller reports that (7008) and keeps the site `any` rather than
/// inventing a shape; it must not fall back to the link kind, because after a
/// `.*` the value is the row, not the link.
pub(crate) fn splat_kind(current: &Kind, schema: &SchemaIndex) -> Result<Kind, String> {
    match current {
        // A bare `record` names no table, so the row it splats to is an
        // object whose shape is genuinely open — not unknown, just untyped.
        Kind::Record(targets) if targets.is_empty() => Ok(Kind::Object),
        Kind::Record(targets) => {
            let mut rows = Vec::with_capacity(targets.len());
            for target in targets {
                let name = target.to_string();
                let table = schema
                    .tables
                    .get(&name)
                    .filter(|table| !table.fields.is_empty())
                    .ok_or(name)?;
                rows.push(crate::analyzer::data::select::object_kind_for_all_fields(
                    table,
                ));
            }
            Ok(Kind::either(rows))
        }
        Kind::Array(element, max) => Ok(Kind::Array(Box::new(splat_kind(element, schema)?), *max)),
        Kind::Set(element, max) => Ok(Kind::Set(Box::new(splat_kind(element, schema)?), *max)),
        _ => Ok(current.clone()),
    }
}

/// The table a `.*` on `kind` cannot expand, when there is one — the
/// reporting half of [`splat_kind`].
pub(crate) fn unexpandable_splat_table(kind: &Kind, schema: &SchemaIndex) -> Option<String> {
    splat_kind(kind, schema).err()
}

/// One stepping rule shared by pure inference and position checking.
fn step_part_kind(
    current: &Kind,
    part: &ast::IdiomPart,
    ctx: &mut AnalysisContext<'_>,
) -> Option<Kind> {
    match part {
        ast::IdiomPart::Field(name) => field_of_kind(current, name, ctx.schema()),
        ast::IdiomPart::Index(_) | ast::IdiomPart::Last => collection_element_kind(current),
        // A `.*` that cannot expand leaves the value shapeless: the row is
        // not the link, so `Any` — never the receiver — is the honest answer.
        ast::IdiomPart::All => Some(splat_kind(current, ctx.schema()).unwrap_or(Kind::Any)),
        ast::IdiomPart::Where(_) | ast::IdiomPart::Optional => Some(current.clone()),
        // `...` flattens one level of nesting: an `array<array<T>>` becomes an
        // `array<T>`; anything else is left as it stands.
        ast::IdiomPart::Flatten => Some(match current {
            Kind::Array(element, _) if matches!(**element, Kind::Array(..) | Kind::Set(..)) => {
                (**element).clone()
            }
            other => other.clone(),
        }),
        ast::IdiomPart::Method { name, args } => {
            let arg_kinds: Vec<Kind> = std::iter::once(current.clone())
                .chain(
                    args.iter()
                        .map(|arg| infer_expression_fact(arg, ctx).kind.unwrap_or(Kind::Any)),
                )
                .collect();
            method_return_kind(current, &name.node, &arg_kinds, args, ctx)
        }
        // A destructure names its own keys, so the object it builds is known
        // even when a selected path is not: the key is there, its kind is
        // `any`, and the missing field is reported on its own (1002). Losing
        // the whole object over one unresolved key was how `author.{nope}`
        // came back as a bare `any`.
        ast::IdiomPart::Destructure(selected) => {
            let mut fields = std::collections::BTreeMap::new();
            for sub in selected {
                let segments = plain_field_segments(&sub.node)?;
                let mut kind = Some(current.clone());
                for segment in &segments {
                    kind = kind.and_then(|kind| field_of_kind(&kind, segment, ctx.schema()));
                }
                fields.insert(segments.join("."), kind.unwrap_or(Kind::Any));
            }
            Some(Kind::Literal(KindLiteral::Object(fields)))
        }
        // A traversal step is a step like any other: it takes the records in
        // hand and answers the records it reaches. Modelling it here is what
        // puts everything BEHIND a `->` inside the walk — and therefore inside
        // every check the walk drives — instead of behind a per-form
        // recognizer that has to be remembered.
        ast::IdiomPart::Graph { dir, step } => {
            crate::analyzer::data::select::graph_step_kind(current, dir.node, step, ctx.schema())
        }
        // `Start` is a value, not a step off one; `Recurse` and `Partial` name
        // no single reachable shape.
        ast::IdiomPart::Start(_) | ast::IdiomPart::Recurse { .. } | ast::IdiomPart::Partial(_) => {
            None
        }
    }
}

/// Checking-side view of method dispatch: the resolved return kind, or
/// `None` when the receiver's kind family has no such method.
pub fn method_result(
    receiver: &Kind,
    method: &str,
    args: &[Kind],
    arg_exprs: &[ast::Spanned<ast::Expr>],
    ctx: &mut AnalysisContext<'_>,
) -> Option<Kind> {
    method_return_kind(receiver, method, args, arg_exprs, ctx)
}

/// Steps a base kind through a chain of field segments via the schema —
/// `record<file>` then `folder` reaches `option<record<folder>>`. Used to
/// resolve a guarded field path's declared kind before narrowing it.
pub(crate) fn step_field_path(
    base: &Kind,
    fields: &[String],
    schema: &SchemaIndex,
) -> Option<Kind> {
    crate::kinds::project_fields(base, fields, Some(schema))
}

fn step_idiom_kind(idiom: &ast::Idiom, ctx: &mut AnalysisContext<'_>) -> Option<Kind> {
    // A flow narrowing is looked up inside the walk below, at every prefix —
    // see `narrowed_kind`.
    let mut parts = idiom.parts.iter();
    let first = parts.next()?;
    let mut place = start_place(&first.node);
    let mut current: Option<Kind> = match &first.node {
        ast::IdiomPart::Start(expr) => infer_expression_fact(expr, ctx).kind,
        ast::IdiomPart::Field(name) => ctx.row_table().and_then(|table| {
            crate::analyzer::data::select::kind_for_path(table, std::slice::from_ref(name))
        }),
        _ => return None,
    };
    if let Some(narrowed) = narrowed_kind(place.as_ref(), ctx.env()) {
        current = Some(narrowed);
    }

    for part in parts {
        let stepped = current.and_then(|kind| step_part_kind(&kind, &part.node, ctx));
        place = place.and_then(|prefix| prefix.stepped(&part.node));
        // A narrowing at this prefix supersedes the stepped kind — and stands
        // in for it when stepping failed, which is how an optional
        // intermediate segment stops killing the whole path.
        current = Some(narrowed_kind(place.as_ref(), ctx.env()).or(stepped)?);
    }
    current
}

/// The kind of `value.field`: [`crate::kinds::project`] with the schema in
/// hand, so record links resolve.
pub(crate) fn field_of_kind(value: &Kind, field: &str, schema: &SchemaIndex) -> Option<Kind> {
    crate::kinds::project(
        value,
        &crate::schema::FieldStep::Field(field.to_string()),
        Some(schema),
    )
}

/// Method calls dispatch to the receiver kind's function family:
/// `array.len()` is `array::len(array)`, `name.len()` is `string::len(name)`.
/// Receivers whose family has no such method fall through to the generic
/// methods every value answers (see [`generic_method_path`]).
///
/// `arg_exprs` are the method's *argument expressions*, which the kind-only
/// `args` cannot stand in for: a closure argument's contribution is its body,
/// so `array::map`/`fold`/`reduce` (and `.chain`) read it directly. Passing
/// them is what lets `$rows.map(|$o| $o.name)` resolve at all.
fn method_return_kind(
    receiver: &Kind,
    method: &str,
    args: &[Kind],
    arg_exprs: &[ast::Spanned<ast::Expr>],
    ctx: &mut AnalysisContext<'_>,
) -> Option<Kind> {
    let base = crate::kinds::literal_base_kind(receiver).unwrap_or_else(|| receiver.clone());
    // A union receiver resolves the method on *every* arm and unions the
    // results: `[[1, 2], [3]]` is `array<int, 2> | array<int, 1>`, and
    // `.len()` is defined on both. One arm without the method leaves the
    // call unresolved, as it should — `array<int> | int` has no `.len()`.
    if let Kind::Either(variants) = &base {
        let mut results = Vec::with_capacity(variants.len());
        for variant in variants {
            let mut variant_args = args.to_vec();
            if let Some(first) = variant_args.first_mut() {
                *first = variant.clone();
            }
            results.push(method_return_kind(
                variant,
                method,
                &variant_args,
                arg_exprs,
                ctx,
            )?);
        }
        return Some(Kind::either(results));
    }
    let family = match base {
        Kind::Array(_, _) => Some("array"),
        Kind::Set(_, _) => Some("set"),
        Kind::String => Some("string"),
        Kind::Object => Some("object"),
        Kind::Duration => Some("duration"),
        Kind::Datetime => Some("time"),
        Kind::Bytes => Some("bytes"),
        Kind::Record(_) => Some("record"),
        Kind::Int | Kind::Float | Kind::Decimal | Kind::Number => Some("math"),
        // Kinds with no function family of their own (`bool`, `uuid`,
        // `geometry`, `range`, …) still take the generic methods below.
        _ => None,
    };
    if let Some(family) = family {
        if let Some(kind) =
            builtin_method_kind(&format!("{family}::{method}"), args, arg_exprs, ctx)
        {
            return Some(kind);
        }
    }
    // A `set` receiver also answers two methods its function family doesn't
    // name, borrowed from `array` (verified on SurrealDB 3.0.5: `.every()`
    // and `.includes()` dispatch, while the rest of `array`'s surface —
    // `.sort()`, `.push()`, `.distinct()`, … — does not).
    if matches!(base, Kind::Set(_, _)) && matches!(method, "every" | "includes") {
        if let Some(kind) = builtin_method_kind(&format!("array::{method}"), args, arg_exprs, ctx) {
            return Some(kind);
        }
    }
    // `.chain(|$v| …)` answers on every receiver but has no function-family
    // twin — there is no `value::chain` builtin for the signature table to
    // describe. Its contract is exactly "pipe the receiver into the closure",
    // so the return kind is the closure's, derived from the body with the
    // parameter bound to the receiver.
    if method == "chain" {
        let closure = arg_exprs.first().and_then(|arg| match &arg.node {
            ast::Expr::Closure(closure) => Some(closure),
            _ => None,
        });
        return Some(match closure {
            Some(closure) => closure_return_kind(closure, std::slice::from_ref(receiver), ctx)
                .unwrap_or(Kind::Any),
            // A non-closure argument violates `.chain`'s contract; that is not
            // "no such method", so the call still resolves and the value stays
            // unknown rather than being invented.
            None => Kind::Any,
        });
    }
    builtin_method_kind(generic_method_path(method)?.as_str(), args, arg_exprs, ctx)
}

/// The kind a builtin resolves to when called as a method, or `None` when no
/// builtin of that name exists.
///
/// Resolution and inference are separate questions. Several built-ins return an
/// honest `Kind::Any` — `record::id` (a record id is genuinely one of many
/// shapes), `array::at` over an `array<any>` — so treating `any` as "no such
/// method" made `$r.id()` a false `E5001`. Existence is asked of the builtin
/// catalog, which is proven to match the dispatch tables arm-for-arm; the
/// resolved kind is whatever the analyzer produced, `any` included.
fn builtin_method_kind(
    path: &str,
    args: &[Kind],
    arg_exprs: &[ast::Spanned<ast::Expr>],
    ctx: &mut AnalysisContext<'_>,
) -> Option<Kind> {
    if !crate::analyzer::function::is_builtin(path) {
        return None;
    }
    let call = crate::analyzer::function::synthetic_method_call(path, arg_exprs);
    Some(crate::analyzer::function::analyze_builtin_function(
        ctx, &call, args,
    ))
}

/// Methods SurrealDB answers on *every* receiver, whatever its kind —
/// verified on 3.0.5 across string/int/float/decimal/array/set/object/
/// duration/datetime/bytes/uuid/bool/record/range receivers. They are the
/// method spellings of the `type::`/`value::` builtins, plus `repeat`, which
/// falls back to `array::repeat` for receivers whose own family has no
/// `repeat` (`(5).repeat(2)` is `[5, 5]`).
///
/// `.chain(|$v| …)` is the one generic method with no builtin behind it; it is
/// resolved directly in [`method_return_kind`] from its closure's body.
fn generic_method_path(method: &str) -> Option<String> {
    Some(match method {
        "to_string" => "type::string".to_string(),
        "type_of" => "type::of".to_string(),
        "diff" => "value::diff".to_string(),
        "patch" => "value::patch".to_string(),
        "repeat" => "array::repeat".to_string(),
        // `type::is_*` predicates answer on any value; an `is_` name with no
        // such builtin resolves to nothing, as it should.
        other if other.starts_with("is_") => format!("type::{other}"),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Compound expressions
// ---------------------------------------------------------------------------

fn binary_fact(
    lhs: &ast::Spanned<ast::Expr>,
    op: &ast::Spanned<ast::BinaryOp>,
    rhs: &ast::Spanned<ast::Expr>,
    span: SourceSpan,
    ctx: &mut AnalysisContext<'_>,
) -> ExpressionFact {
    let lhs_fact = infer_expression_fact(lhs, ctx);
    // Occurrence typing over a short-circuit operator: SurrealDB evaluates the
    // right operand of `A AND B` only where `A` held, and of `A OR B` only
    // where it did not. Either way the right operand is inferred (and any
    // function-call arguments inside it) with what the left proves applied to a
    // scoped child env, so `(x != NONE) AND f(x)`, `(subject IN $a) AND
    // fn::test(subject)`, `$x IS NONE OR f($x)` and chains `A AND B AND C` all
    // read the narrowed subject. The child env is discarded after, so the
    // narrowing never leaks past the operator into the surrounding scope.
    let rhs_fact = match short_circuit_facts(&op.node, &lhs.node, ctx.env()) {
        // A child env is forked only where the left proves something — an
        // operator that narrows nothing infers its right operand in the parent
        // env unchanged.
        Some(facts) => ctx.with_child_env(|ctx| {
            crate::analyzer::flow::narrow::apply_facts(ctx, &facts);
            infer_expression_fact(rhs, ctx)
        }),
        None => infer_expression_fact(rhs, ctx),
    };

    let mut fact = ExpressionFact::new(span, ExpressionValueClass::Unknown);
    merge_dependencies(&mut fact, lhs_fact.dependencies);
    merge_dependencies(&mut fact, rhs_fact.dependencies);
    fact.partial.extend(lhs_fact.partial.iter().cloned());
    fact.partial.extend(rhs_fact.partial.iter().cloned());

    if let (Some(lhs_kind), Some(rhs_kind)) = (&lhs_fact.kind, &rhs_fact.kind) {
        if let Some(kind) = binary_result_kind(&op.node, lhs_kind, rhs_kind) {
            fact.kind = Some(kind);
            fact.partial.clear();
            return fact;
        }
    }

    fact.with_partial(PartialReason::UnsupportedSyntax("BinaryExpression".into()))
}

/// What the **left** operand of a short-circuit operator proves about the
/// region the **right** one runs in, or `None` when it proves nothing.
///
/// One rule for both operators, and it is the definition of short-circuiting:
/// `A AND B` evaluates `B` only where `A` held, `A OR B` only where it did not.
/// So the region is `guard_of(A, polarity)` with `polarity` the operator's own
/// truth value — which is why this replaces both the `AND`-only effect list and
/// the separate `= NONE OR` recognizer beside it, and why every spelling the
/// guard IR knows now works on both sides (`$x IS NONE OR f($x)`,
/// `!($x != NONE) OR f($x)`, `type::is_none($x) OR f($x)`).
///
/// Both operands are still read left-to-right only: `B` narrows nothing about
/// `A`, because `A` was already evaluated.
pub(crate) fn short_circuit_facts(
    op: &ast::BinaryOp,
    lhs: &ast::Expr,
    env: &crate::statement_env::StatementEnv,
) -> Option<crate::analyzer::facts::Facts> {
    let polarity = match op {
        ast::BinaryOp::And => true,
        ast::BinaryOp::Or => false,
        _ => return None,
    };
    let facts = crate::analyzer::flow::narrow::guard_facts(lhs, polarity, env);
    (!facts.is_empty()).then_some(facts)
}

/// Drops the `none`/`null` variants from a union: `none | string` becomes
/// `string`.
///
/// This is the **coalesce** rule, not a guard rule: `??` falls through on both
/// sentinels (`NONE ?? 'd'` and `NULL ?? 'd'` both yield `'d'` on the engine),
/// so both go. A `!= NONE` guard eliminates only `none` — that is
/// [`Refinement::Without(Kind::None)`]. A non-union kind is returned unchanged.
///
/// [`Refinement::Without(Kind::None)`]: crate::analyzer::facts::Refinement
pub(crate) fn narrow_out_none(kind: &Kind) -> Kind {
    let Kind::Either(variants) = kind else {
        return kind.clone();
    };
    let kept: Vec<Kind> = variants
        .iter()
        .filter(|v| !matches!(v, Kind::None | Kind::Null))
        .cloned()
        .collect();
    match kept.len() {
        0 => kind.clone(),
        1 => kept.into_iter().next().expect("one variant"),
        _ => Kind::Either(kept),
    }
}

fn prefix_fact(
    op: &ast::Spanned<ast::PrefixOp>,
    operand: &ast::Spanned<ast::Expr>,
    span: SourceSpan,
    ctx: &mut AnalysisContext<'_>,
) -> ExpressionFact {
    let operand_fact = infer_expression_fact(operand, ctx);
    let mut fact = ExpressionFact::new(span, ExpressionValueClass::Unknown);
    fact.dependencies = operand_fact.dependencies;
    fact.partial = operand_fact.partial;

    match (&op.node, &operand_fact.kind) {
        // `!` negates truthiness, so it is a bool for any operand.
        (ast::PrefixOp::Not, _) => {
            fact.kind = Some(Kind::Bool);
            fact.partial.clear();
            fact
        }
        (ast::PrefixOp::Neg | ast::PrefixOp::Pos, Some(kind)) if is_numeric(kind) => {
            fact.kind = Some(kind.clone());
            fact.partial.clear();
            fact
        }
        _ => fact.with_partial(PartialReason::UnsupportedSyntax("PrefixExpression".into())),
    }
}

fn object_fact(
    fields: &[(ast::Spanned<String>, ast::Spanned<ast::Expr>)],
    span: SourceSpan,
    ctx: &mut AnalysisContext<'_>,
) -> ExpressionFact {
    let mut kinds = std::collections::BTreeMap::new();
    let mut partial = Vec::new();

    for (key, value) in fields {
        let value_fact = infer_expression_fact(value, ctx);
        partial.extend(value_fact.partial.iter().cloned());
        let kind = object_property_kind(&value_fact);
        kinds.insert(key.node.clone(), kind);
    }

    let mut fact = ExpressionFact::new(span, ExpressionValueClass::Object)
        .with_kind(Kind::Literal(KindLiteral::Object(kinds)));
    fact.partial = partial;
    let values: Option<std::collections::BTreeMap<String, surrealdb_types::Value>> = fields
        .iter()
        .map(|(key, value)| {
            infer_expression_fact(value, ctx)
                .value
                .map(|value| (key.node.clone(), value))
        })
        .collect();
    if let Some(values) = values {
        fact.value = Some(surrealdb_types::Value::Object(values.into()));
    }
    fact
}

/// The kind an object-literal property contributes to the object's type. A
/// property written as a constant string is given its *literal* kind
/// (`'marble'` rather than the widened `string`) so a DEFAULT/CONTENT object
/// type-checks against a field whose object type constrains that property to
/// a string-literal union (`'marble' | 'euclid' | ...`). Other kinds pass
/// through unchanged.
pub(crate) fn object_property_kind(value_fact: &ExpressionFact) -> Kind {
    match (&value_fact.kind, &value_fact.value) {
        (Some(Kind::String), Some(surrealdb_types::Value::String(text))) => {
            Kind::Literal(KindLiteral::String(text.clone()))
        }
        (Some(kind), _) => kind.clone(),
        (None, _) => Kind::Any,
    }
}

fn array_fact(
    elements: &[ast::Spanned<ast::Expr>],
    span: SourceSpan,
    ctx: &mut AnalysisContext<'_>,
) -> ExpressionFact {
    let facts: Vec<_> = elements
        .iter()
        .map(|element| infer_expression_fact(element, ctx))
        .collect();
    let max_len = Some(facts.len() as u64);

    // The element type is the union of the element kinds: a single distinct
    // kind collapses to `array<T>` (`Kind::either` dedupes), disagreeing kinds
    // become `array<T | U>`. An element of unknown kind (`Any`) poisons the
    // whole element type — no union can be narrower than `any`. An empty array
    // (or one whose elements are all unresolved) keeps the `array<any>` default.
    let element_kinds: Vec<Kind> = facts.iter().filter_map(|f| f.kind.clone()).collect();
    let element_kind: Option<Kind> = if element_kinds.is_empty() {
        None
    } else if element_kinds.iter().any(|kind| matches!(kind, Kind::Any)) {
        Some(Kind::Any)
    } else {
        Some(Kind::either(element_kinds))
    };

    let mut fact = ExpressionFact::new(span, ExpressionValueClass::Array).with_kind(Kind::Array(
        Box::new(element_kind.clone().unwrap_or(Kind::Any)),
        max_len,
    ));
    fact.partial = facts
        .iter()
        .flat_map(|f| f.partial.iter().cloned())
        .collect();
    if let Some(values) = facts
        .iter()
        .map(|f| f.value.clone())
        .collect::<Option<Vec<_>>>()
    {
        fact.value = Some(surrealdb_types::Value::Array(values.into()));
    }
    fact
}

fn call_fact(call: &ast::Call, span: SourceSpan, ctx: &mut AnalysisContext<'_>) -> ExpressionFact {
    let mut fact = ExpressionFact::new(span, ExpressionValueClass::FunctionCall);
    fact.dependencies.function = Some(call.path.node.clone());

    // A function that reads only its argument's cardinality puts that argument
    // in a position where an ungrouped `SELECT count()` is right and `GROUP
    // ALL` is not the same query — see `reads_only_cardinality`.
    let cardinality =
        crate::analyzer::data::select::reads_only_cardinality(call.path.node.as_str());
    let args: Vec<Kind> = ctx.with_cardinality_position(cardinality, |ctx| {
        call.args
            .iter()
            .map(|arg| infer_expression_fact(arg, ctx).kind.unwrap_or(Kind::Any))
            .collect()
    });
    let kind = crate::analyzer::function::analyze_builtin_function(ctx, call, &args);
    fact.with_kind(kind)
}

fn cast_fact(
    ty: &ast::Spanned<ast::TypeExpr>,
    inner: &ast::Spanned<ast::Expr>,
    span: SourceSpan,
    ctx: &mut AnalysisContext<'_>,
) -> ExpressionFact {
    let fact = ExpressionFact::new(span, ExpressionValueClass::Unknown);
    let Some(target) = cast_kind(&ty.node) else {
        return fact.with_partial(PartialReason::UnsupportedSyntax("TypeCast".into()));
    };
    // A bare `<array>` / `<set>` names no element kind, and the engine keeps
    // the operand's elements as they are — so a collection operand's element
    // kind survives the cast (`<set> $tags` over `array<string>` is
    // `set<string>`, not `set<any>`).
    let kind = match &target {
        Kind::Array(element, _) | Kind::Set(element, _) if **element == Kind::Any => {
            match infer_expression_fact(inner, ctx).kind {
                Some(Kind::Array(element, _) | Kind::Set(element, _)) => match target {
                    Kind::Set(_, _) => Kind::Set(element, None),
                    _ => Kind::Array(element, None),
                },
                _ => target,
            }
        }
        _ => target,
    };
    fact.with_kind(kind)
}

/// The kind a cast target names, shared with the checking side.
pub fn cast_target_kind(ty: &ast::TypeExpr) -> Option<Kind> {
    cast_kind(ty)
}

/// The kind a cast target names: every type a `DEFINE FIELD` can declare is
/// a type a value can be cast to, so this is the schema's type reader. A
/// name it does not know (`<future>`, `<regex>`) is `None`, which leaves the
/// cast's kind unknown rather than guessed.
fn cast_kind(ty: &ast::TypeExpr) -> Option<Kind> {
    crate::schema::kind_from_type_expr(ty, "").kind
}

/// Mirrors SurrealDB's `TryAdd`/`TrySub`/`TryMul` on `Value`: numeric
/// arithmetic, string/collection concatenation, and temporal arithmetic
/// (`datetime ± duration`, `datetime - datetime`, `duration ± duration`,
/// `duration * int`).
pub fn binary_result_kind(op: &ast::BinaryOp, lhs: &Kind, rhs: &Kind) -> Option<Kind> {
    use ast::BinaryOp as Op;
    // `??` yields its left operand only when that operand is *not* NONE/NULL,
    // so the NONE variant of an `option<T>` can never reach the result. Strip
    // it up front: an `option<string>` is `Either([none, string])`, which
    // matches none of the NullCoalesce arms below and would otherwise fall to
    // the catch-all that unions the NONE straight back in — leaving
    // `(opt ?? 'd') + '!'` a false E2004. `narrow_out_none` is a no-op on
    // `Kind::Any`, on bare `Kind::None`, and on a union that would empty out,
    // so `any ?? x` and `NONE ?? x` keep their existing behaviour.
    let coalesce_lhs;
    let raw_lhs = lhs;
    let lhs = if matches!(op, Op::NullCoalesce) {
        coalesce_lhs = narrow_out_none(lhs);
        &coalesce_lhs
    } else {
        lhs
    };
    match op {
        Op::Add if matches!(lhs, Kind::String) && matches!(rhs, Kind::String) => Some(Kind::String),
        Op::Add | Op::Sub if matches!(lhs, Kind::Duration) && matches!(rhs, Kind::Duration) => {
            Some(Kind::Duration)
        }
        Op::Add | Op::Sub
            if matches!(
                (lhs, rhs),
                (Kind::Datetime, Kind::Duration) | (Kind::Duration, Kind::Datetime)
            ) =>
        {
            Some(Kind::Datetime)
        }
        Op::Sub if matches!(lhs, Kind::Datetime) && matches!(rhs, Kind::Datetime) => {
            Some(Kind::Duration)
        }
        Op::Mul if matches!(lhs, Kind::Duration) && matches!(rhs, Kind::Int) => {
            Some(Kind::Duration)
        }
        // Collection concatenation and difference.
        Op::Add | Op::Sub
            if matches!(lhs, Kind::Array(_, _) | Kind::Set(_, _))
                && matches!(rhs, Kind::Array(_, _) | Kind::Set(_, _)) =>
        {
            let (Kind::Array(a, _) | Kind::Set(a, _)) = lhs else {
                return None;
            };
            let (Kind::Array(b, _) | Kind::Set(b, _)) = rhs else {
                return None;
            };
            // The element of a concatenation is the join of the two elements.
            let element = Box::new(crate::lattice::join(a, b));
            Some(match lhs {
                Kind::Set(_, _) => Kind::Set(element, None),
                _ => Kind::Array(element, None),
            })
        }
        Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Rem | Op::Pow
            if is_numeric(lhs) && is_numeric(rhs) =>
        {
            Some(numeric_result(lhs, rhs))
        }
        // A comparison produces a bool no matter what it compares; mismatched
        // operands violate an invariant, not the result type. The same holds
        // for every membership, containment, geometry, fuzzy-match, full-text
        // and KNN operator.
        Op::Eq
        | Op::Exact
        | Op::NotEq
        | Op::Lt
        | Op::LtEq
        | Op::Gt
        | Op::GtEq
        | Op::Is
        | Op::IsNot
        | Op::In
        | Op::NotIn
        | Op::Contains
        | Op::ContainsNot
        | Op::ContainsAll
        | Op::ContainsAny
        | Op::ContainsNone
        | Op::Inside
        | Op::NotInside
        | Op::AllInside
        | Op::AnyInside
        | Op::NoneInside
        | Op::Outside
        | Op::Intersects
        | Op::Match
        | Op::NotMatch
        | Op::AllMatch
        | Op::AnyMatch
        | Op::AnyEq
        | Op::AllEq
        | Op::Matches(_)
        | Op::Knn(_) => Some(Kind::Bool),
        Op::And | Op::Or if matches!(lhs, Kind::Bool) && matches!(rhs, Kind::Bool) => {
            Some(Kind::Bool)
        }
        // `a ?: b` yields whichever side is truthy, so it is one of the two.
        Op::TruthyCoalesce => Some(Kind::either(vec![raw_lhs.clone(), rhs.clone()])),
        Op::NullCoalesce if matches!(lhs, Kind::None | Kind::Null) => Some(rhs.clone()),
        // `x ?? NONE` is `x`: the default only surfaces when `x` is already
        // NONE, so the NONE variant survives — report the *unstripped* left.
        Op::NullCoalesce if matches!(rhs, Kind::None | Kind::Null) => Some(raw_lhs.clone()),
        // `x ?? []`: an empty-array literal default (max length 0) carries no
        // element constraint, so coalescing keeps the other side's collection
        // type (`->edge->target ?? []` stays `array<record<target>>`).
        Op::NullCoalesce
            if is_empty_array_literal(rhs)
                && matches!(lhs, Kind::Array(_, _) | Kind::Set(_, _)) =>
        {
            Some(lhs.clone())
        }
        Op::NullCoalesce
            if is_empty_array_literal(lhs)
                && matches!(rhs, Kind::Array(_, _) | Kind::Set(_, _)) =>
        {
            Some(rhs.clone())
        }
        // `numeric ?? numeric` (e.g. `math::sum(...) ?? 0`) stays numeric rather
        // than becoming a `number | int` union: every numeric kind is a
        // `number`, so the coalesced value is numeric and downstream arithmetic
        // (`$this.total_on_hand - $this.total_committed`) keeps resolving.
        Op::NullCoalesce if is_numeric(lhs) && is_numeric(rhs) => Some(if lhs == rhs {
            lhs.clone()
        } else {
            Kind::Number
        }),
        // Coalescing two known kinds yields one of them.
        Op::NullCoalesce => Some(Kind::either(vec![lhs.clone(), rhs.clone()])),
        _ => None,
    }
}

/// An empty array literal (`[]`): an `array` whose max length is exactly 0.
/// This is the shape [`array_fact`] produces for an empty literal, and a
/// zero-length array default (`?? []`) constrains no element type.
fn is_empty_array_literal(kind: &Kind) -> bool {
    matches!(kind, Kind::Array(_, Some(0)))
}

fn numeric_result(lhs: &Kind, rhs: &Kind) -> Kind {
    if matches!(lhs, Kind::Decimal) || matches!(rhs, Kind::Decimal) {
        Kind::Decimal
    } else if matches!(lhs, Kind::Float) || matches!(rhs, Kind::Float) {
        Kind::Float
    } else if matches!(lhs, Kind::Number) || matches!(rhs, Kind::Number) {
        // `number` is `int | float | decimal`, so an operand that *may* be a
        // float makes the result one. Falling through to `Int` here claimed a
        // kind the value need not have — `<number> * <int>` was typed `int`,
        // which a `TYPE number` column can violate at runtime.
        Kind::Number
    } else {
        Kind::Int
    }
}

pub(crate) use crate::kinds::is_numeric;

fn merge_dependencies(fact: &mut ExpressionFact, deps: crate::expression::ExpressionDependencies) {
    fact.dependencies.field_paths.extend(deps.field_paths);
    fact.dependencies.variables.extend(deps.variables);
    fact.dependencies.params.extend(deps.params);
}

fn scalar_fact(span: SourceSpan, class: ExpressionValueClass, kind: Kind) -> ExpressionFact {
    ExpressionFact::new(span, class).with_kind(kind)
}

fn partial_fact(span: SourceSpan, class: ExpressionValueClass, syntax: String) -> ExpressionFact {
    ExpressionFact::new(span, class).with_partial(PartialReason::UnsupportedSyntax(syntax))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::TableDef;
    use crate::statement_env::StatementEnv;
    use surrealguard_syntax::parse::{parse_source, ParsedSource};
    use surrealguard_syntax::source::SourceId;

    use crate::schema::{extract_schema, SchemaIndex};

    fn parse(query: &str) -> ParsedSource {
        parse_source(SourceId::new("infer:test"), query).expect("test query parses")
    }

    fn infer_first(
        parsed: &ParsedSource,
        kind: &str,
        row_table: Option<&TableDef>,
        env: &StatementEnv,
    ) -> ExpressionFact {
        infer_first_with_schema(parsed, kind, &SchemaIndex::default(), row_table, env)
    }

    fn infer_first_with_schema(
        parsed: &ParsedSource,
        kind: &str,
        schema: &SchemaIndex,
        row_table: Option<&TableDef>,
        env: &StatementEnv,
    ) -> ExpressionFact {
        let expr = surrealguard_syntax::lower::lower_first_expr(parsed, kind)
            .unwrap_or_else(|| panic!("no {kind} in {:?}", parsed.text()));
        let mut diagnostics: Vec<surrealguard_diagnostics::Finding> = Vec::new();
        let mut ctx = AnalysisContext::scoped(
            schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
            env.clone(),
            row_table,
        );
        infer_expression_fact(&expr, &mut ctx)
    }

    fn person_schema() -> SchemaIndex {
        let parsed = parse_source(
            SourceId::new("infer:schema"),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD profile.email ON person TYPE string;",
        )
        .expect("schema parses");
        extract_schema(&[parsed]).schema
    }

    #[test]
    fn infers_literal_kinds_including_prefixed_strings() {
        let env = StatementEnv::default();
        let cases = [
            ("RETURN 42;", "Number", Kind::Int),
            ("RETURN 2.5;", "Number", Kind::Float),
            ("RETURN 'hi';", "String", Kind::String),
            ("RETURN d'2024-01-01T00:00:00Z';", "String", Kind::Datetime),
            ("RETURN u'0189-aa';", "String", Kind::Uuid),
            ("RETURN true;", "Bool", Kind::Bool),
            ("RETURN NONE;", "None", Kind::None),
            ("RETURN null;", "None", Kind::Null),
            ("RETURN 1h;", "Duration", Kind::Duration),
        ];

        for (query, node_kind, expected) in cases {
            let parsed = parse(query);
            let fact = infer_first(&parsed, node_kind, None, &env);
            assert_eq!(fact.kind, Some(expected), "query: {query}");
            assert!(fact.partial.is_empty(), "query: {query}");
        }
    }

    #[test]
    fn resolves_schema_backed_field_paths_including_nested() {
        let schema = person_schema();
        let table = schema.tables.get("person").unwrap();
        let env = StatementEnv::default();

        let parsed = parse("SELECT profile.email FROM person;");
        let fact = infer_first(&parsed, "Path", Some(table), &env);

        assert_eq!(fact.kind, Some(Kind::String));
        assert_eq!(fact.dependencies.field_paths, vec!["profile.email"]);
        assert!(fact.partial.is_empty());
    }

    #[test]
    fn unbound_params_are_recorded_dependencies_with_explicit_unresolved() {
        let env = StatementEnv::default();
        let parsed = parse("RETURN $name;");

        let fact = infer_first(&parsed, "VariableName", None, &env);

        assert_eq!(fact.kind, None);
        assert_eq!(fact.dependencies.params, vec!["name"]);
        assert_eq!(fact.partial, vec![PartialReason::Unresolved]);
    }

    #[test]
    fn let_bound_params_resolve_through_binary_expressions() {
        let mut env = StatementEnv::default();
        let parsed = parse("RETURN $age + 1;");
        env.define_let(
            "age".into(),
            ExpressionFact::new(
                SourceSpan::new(
                    parsed.source_id().clone(),
                    surrealguard_syntax::span::ByteRange::new(0, 1).unwrap(),
                ),
                ExpressionValueClass::Literal,
            )
            .with_kind(Kind::Int),
        );

        let fact = infer_first(&parsed, "BinaryExpression", None, &env);

        assert_eq!(fact.kind, Some(Kind::Int));
        assert!(fact.partial.is_empty());
    }

    #[test]
    fn comparison_boolean_and_coalesce_operators_infer_kinds() {
        let schema = person_schema();
        let table = schema.tables.get("person").unwrap();
        let env = StatementEnv::default();

        let ge = parse("SELECT age >= 18 AS adult FROM person;");
        let fact = infer_first(&ge, "BinaryExpression", Some(table), &env);
        assert_eq!(fact.kind, Some(Kind::Bool));

        let and = parse("RETURN true AND false;");
        let fact = infer_first(&and, "BinaryExpression", None, &env);
        assert_eq!(fact.kind, Some(Kind::Bool));

        let coalesce = parse("RETURN NONE ?? 'fallback';");
        let fact = infer_first(&coalesce, "BinaryExpression", None, &env);
        assert_eq!(fact.kind, Some(Kind::String));

        // Mismatched comparisons still produce a bool: the mismatch is an
        // invariant violation, not type ambiguity.
        let mismatched = parse("RETURN 'a' > 18;");
        let fact = infer_first(&mismatched, "BinaryExpression", None, &env);
        assert_eq!(fact.kind, Some(Kind::Bool));

        // Coalescing two known kinds yields one of them.
        let union = parse("RETURN 1 ?? 'fallback';");
        let fact = infer_first(&union, "BinaryExpression", None, &env);
        assert_eq!(fact.kind, Some(Kind::Either(vec![Kind::Int, Kind::String])));
    }

    #[test]
    fn coalesce_with_empty_array_default_keeps_the_collection_type() {
        let env = StatementEnv::default();

        // `[1, 2] ?? []` — the empty-array default carries no element
        // constraint, so the result stays `array<int>` rather than widening.
        let coal = parse("RETURN [1, 2] ?? [];");
        let fact = infer_first(&coal, "BinaryExpression", None, &env);
        assert_eq!(fact.kind, Some(Kind::Array(Box::new(Kind::Int), Some(2))));
    }

    #[test]
    fn coalesce_of_two_numerics_stays_numeric() {
        let env = StatementEnv::default();

        // `1.5 ?? 0` — differing numeric kinds coalesce to `number`, not a
        // `float | int` union, so downstream arithmetic keeps resolving.
        let differ = parse("RETURN 1.5 ?? 0;");
        let fact = infer_first(&differ, "BinaryExpression", None, &env);
        assert_eq!(fact.kind, Some(Kind::Number));

        // Equal numeric kinds coalesce to that kind.
        let same = parse("RETURN 1 ?? 0;");
        let fact = infer_first(&same, "BinaryExpression", None, &env);
        assert_eq!(fact.kind, Some(Kind::Int));
    }

    #[test]
    fn coalesce_strips_none_from_an_option_left_operand() {
        use surrealguard_syntax::ast::BinaryOp;

        // `option<string> ?? 'd'` is a `string`: the default is exactly what
        // surfaces when the left side is NONE, so NONE cannot survive.
        let opt_string = Kind::Either(vec![Kind::None, Kind::String]);
        assert_eq!(
            binary_result_kind(&BinaryOp::NullCoalesce, &opt_string, &Kind::String),
            Some(Kind::String)
        );

        // The purpose-built arms now see the stripped kind too:
        // `option<array<int>> ?? []` keeps the collection type ...
        let opt_array = Kind::Either(vec![Kind::None, Kind::Array(Box::new(Kind::Int), None)]);
        assert_eq!(
            binary_result_kind(
                &BinaryOp::NullCoalesce,
                &opt_array,
                &Kind::Array(Box::new(Kind::Any), Some(0)),
            ),
            Some(Kind::Array(Box::new(Kind::Int), None))
        );
        // ... and `option<int> ?? 0` stays an `int` rather than `int | none`.
        let opt_int = Kind::Either(vec![Kind::None, Kind::Int]);
        assert_eq!(
            binary_result_kind(&BinaryOp::NullCoalesce, &opt_int, &Kind::Int),
            Some(Kind::Int)
        );

        // Unchanged boundaries: a bare NONE left side still yields the
        // default, and an `any` left side is untouched by the narrowing.
        assert_eq!(
            binary_result_kind(&BinaryOp::NullCoalesce, &Kind::None, &Kind::String),
            Some(Kind::String)
        );
        assert_eq!(
            binary_result_kind(&BinaryOp::NullCoalesce, &Kind::Any, &Kind::String),
            Some(Kind::either(vec![Kind::Any, Kind::String]))
        );

        // `x ?? NONE` is `x`: the NONE survives, so the left side is *not*
        // stripped when the default is itself NONE.
        assert_eq!(
            binary_result_kind(&BinaryOp::NullCoalesce, &opt_string, &Kind::None),
            Some(opt_string.clone())
        );
    }

    #[test]
    fn collection_operations_distribute_over_a_union() {
        let int2 = Kind::Array(Box::new(Kind::Int), Some(2));
        let int1 = Kind::Array(Box::new(Kind::Int), Some(1));
        let str1 = Kind::Array(Box::new(Kind::String), Some(1));

        // `[[1, 2], [3]]` — every arm is an array, so it is indexable and
        // its element kind is the union of the arms' elements.
        let same = Kind::Either(vec![int2.clone(), int1.clone()]);
        assert!(is_indexable_kind(&same));
        assert_eq!(collection_element_kind(&same), Some(Kind::Int));

        let mixed = Kind::Either(vec![int2.clone(), str1]);
        assert!(is_indexable_kind(&mixed));
        assert_eq!(
            collection_element_kind(&mixed),
            Some(Kind::either(vec![Kind::Int, Kind::String]))
        );

        // `option<array<T>>` has a definitely-non-collection arm, so the
        // contract is violated — but the element kind still resolves, so
        // inference does not collapse to `any` on the reported expression.
        let optional = Kind::Either(vec![Kind::None, int1]);
        assert!(!is_indexable_kind(&optional));
        assert_eq!(collection_element_kind(&optional), Some(Kind::Int));

        // No arm is a collection: nothing to index, nothing to read.
        let scalar = Kind::Either(vec![Kind::Int, Kind::String]);
        assert!(!is_indexable_kind(&scalar));
        assert_eq!(collection_element_kind(&scalar), None);

        // An `any` arm proves nothing, so it stays permissive.
        assert!(is_indexable_kind(&Kind::Either(vec![Kind::Any, int2])));
    }

    #[test]
    fn prefix_not_and_negation_infer_kinds() {
        let env = StatementEnv::default();

        let not = parse("RETURN !true;");
        let fact = infer_first(&not, "PrefixExpression", None, &env);
        assert_eq!(fact.kind, Some(Kind::Bool));

        // `!` negates truthiness: bool for any operand kind.
        let truthy = parse("RETURN !'text';");
        let fact = infer_first(&truthy, "PrefixExpression", None, &env);
        assert_eq!(fact.kind, Some(Kind::Bool));
    }

    #[test]
    fn objects_and_arrays_infer_structured_kinds() {
        let schema = person_schema();
        let table = schema.tables.get("person").unwrap();
        let env = StatementEnv::default();

        let parsed = parse("SELECT { who: name, tags: ['a', 'b'], ok: true } AS data FROM person;");
        let fact = infer_first(&parsed, "Object", Some(table), &env);

        let Some(Kind::Literal(surrealdb_types::KindLiteral::Object(fields))) = fact.kind else {
            panic!("expected object literal kind, got {:?}", fact.kind);
        };
        assert_eq!(fields["who"], Kind::String);
        assert_eq!(fields["ok"], Kind::Bool);
        assert_eq!(fields["tags"], Kind::Array(Box::new(Kind::String), Some(2)));
    }

    #[test]
    fn array_literals_infer_the_union_of_their_element_kinds() {
        let env = StatementEnv::default();

        // Homogeneous elements collapse to a single element kind.
        let parsed = parse("RETURN [1, 2, 3];");
        let fact = infer_first(&parsed, "Array", None, &env);
        assert_eq!(fact.kind, Some(Kind::Array(Box::new(Kind::Int), Some(3))));
        assert!(fact.partial.is_empty());

        // Mixed elements infer the union `array<int | string>` — no partial.
        let parsed = parse("RETURN [1, 'a'];");
        let fact = infer_first(&parsed, "Array", None, &env);
        assert_eq!(
            fact.kind,
            Some(Kind::Array(
                Box::new(Kind::Either(vec![Kind::Int, Kind::String])),
                Some(2)
            ))
        );
        assert!(
            fact.partial.is_empty(),
            "mixed array no longer punts to a partial: {:?}",
            fact.partial
        );

        // An empty array keeps the `array<any>` default.
        let parsed = parse("RETURN [];");
        let fact = infer_first(&parsed, "Array", None, &env);
        assert_eq!(fact.kind, Some(Kind::Array(Box::new(Kind::Any), Some(0))));
    }

    #[test]
    fn known_function_calls_resolve_return_kinds_by_name() {
        let env = StatementEnv::default();
        let parsed = parse("RETURN string::len($name);");

        let fact = infer_first(&parsed, "FunctionCall", None, &env);

        assert_eq!(fact.kind, Some(Kind::Int));
        assert_eq!(fact.dependencies.function.as_deref(), Some("string::len"));
    }

    #[test]
    fn record_id_ranges_infer_an_array_of_records() {
        let env = StatementEnv::default();
        let parsed = parse("RETURN person:1..5;");
        let fact = infer_first(&parsed, "RecordId", None, &env);
        assert_eq!(
            fact.kind,
            Some(Kind::Array(
                Box::new(Kind::Record(vec!["person".into()])),
                None
            ))
        );
    }

    #[test]
    fn constants_ranges_and_prefixed_literals_infer_their_kinds() {
        let env = StatementEnv::default();
        let cases = [
            ("RETURN MaTh::Pi;", "Constant", Kind::Float),
            ("RETURN time::EPOCH;", "Constant", Kind::Datetime),
            ("RETURN 1..5;", "Range", Kind::Range),
            ("RETURN b'abc';", "String", Kind::Bytes),
            ("RETURN f'bucket:/a';", "String", Kind::File(Vec::new())),
            ("RETURN 1.5f;", "Number", Kind::Float),
            ("RETURN 9.7e-7dec;", "Number", Kind::Decimal),
        ];
        for (query, node, expected) in cases {
            let parsed = parse(query);
            let fact = infer_first(&parsed, node, None, &env);
            assert_eq!(fact.kind, Some(expected), "`{query}`");
            assert!(fact.partial.is_empty(), "`{query}`: {:?}", fact.partial);
        }
        // A path the engine has no constant for is honestly unknown.
        let parsed = parse("RETURN math::nope;");
        let fact = infer_first(&parsed, "Constant", None, &env);
        assert_eq!(fact.kind, None);
        assert!(!fact.partial.is_empty());
    }

    #[test]
    fn every_comparison_family_operator_is_a_bool() {
        let env = StatementEnv::default();
        for op in [
            "==",
            "IS",
            "IS NOT",
            "IN",
            "NOT IN",
            "CONTAINS",
            "CONTAINSALL",
            "INSIDE",
            "NONEINSIDE",
            "OUTSIDE",
            "INTERSECTS",
            "~",
            "!~",
            "?~",
            "?=",
            "*=",
            "@@",
            "@1@",
            "<|3|>",
            "<|3, COSINE|>",
        ] {
            let query = format!("RETURN [1] {op} 1;");
            let parsed = parse(&query);
            let fact = infer_first(&parsed, "BinaryExpression", None, &env);
            assert_eq!(fact.kind, Some(Kind::Bool), "`{query}`");
        }
    }

    #[test]
    fn remainder_power_and_truthy_coalescing_follow_their_operands() {
        let env = StatementEnv::default();
        let cases = [
            ("RETURN 5 % 2;", Kind::Int),
            ("RETURN 2 ** 3;", Kind::Int),
            ("RETURN 2.5 ** 2;", Kind::Float),
            ("RETURN 5.5 % 2;", Kind::Float),
            (
                "RETURN 'a' ?: 1;",
                Kind::either(vec![Kind::String, Kind::Int]),
            ),
        ];
        for (query, expected) in cases {
            let parsed = parse(query);
            let fact = infer_first(&parsed, "BinaryExpression", None, &env);
            assert_eq!(fact.kind, Some(expected), "`{query}`");
        }
    }

    #[test]
    fn record_id_literals_infer_their_record_kind() {
        let env = StatementEnv::default();
        let parsed = parse("RETURN person:one;");

        let fact = infer_first(&parsed, "RecordId", None, &env);

        assert_eq!(fact.kind, Some(Kind::Record(vec!["person".into()])));
        assert!(fact.partial.is_empty());
    }

    #[test]
    fn closures_infer_function_kinds_from_declarations_and_bodies() {
        let env = StatementEnv::default();

        // Declared parameter type + inferred body.
        let parsed = parse("LET $f = |$x: int| $x + 1;");
        let fact = infer_first(&parsed, "Closure", None, &env);
        assert_eq!(
            fact.kind,
            Some(Kind::Function(
                vec![Kind::Int].into(),
                Some(Box::new(Kind::Int))
            ))
        );

        // Declared return type wins without body inference.
        let parsed = parse("LET $f = |$x| -> string { RETURN 'hi'; };");
        let fact = infer_first(&parsed, "Closure", None, &env);
        assert_eq!(
            fact.kind,
            Some(Kind::Function(
                vec![Kind::Any].into(),
                Some(Box::new(Kind::String))
            ))
        );

        // Block bodies infer through LET threading and RETURN.
        let parsed = parse("LET $f = |$x: int| { LET $y = $x * 2; RETURN $y; };");
        let fact = infer_first(&parsed, "Closure", None, &env);
        assert_eq!(
            fact.kind,
            Some(Kind::Function(
                vec![Kind::Int].into(),
                Some(Box::new(Kind::Int))
            ))
        );
    }

    #[test]
    fn casts_infer_the_target_kind() {
        let env = StatementEnv::default();
        let parsed = parse("RETURN <int> '42';");

        let fact = infer_first(&parsed, "TypeCast", None, &env);

        assert_eq!(fact.kind, Some(Kind::Int));
    }

    #[test]
    fn subqueries_infer_their_statement_response_kind() {
        let schema = person_schema();
        let env = StatementEnv::default();

        let sub = parse("RETURN (SELECT name FROM person);");
        let fact = infer_first_with_schema(&sub, "SubQuery", &schema, None, &env);

        assert_eq!(fact.value_class, ExpressionValueClass::Subquery);
        let Some(Kind::Array(element, _)) = fact.kind else {
            panic!("expected array kind, got {:?}", fact.kind);
        };
        let Kind::Literal(surrealdb_types::KindLiteral::Object(fields)) = *element else {
            panic!("expected object literal element");
        };
        assert_eq!(fields["name"], Kind::String);
    }

    #[test]
    fn blocks_stay_partial_in_pure_inference() {
        // Blocks thread an environment through their statements, which is
        // the dispatcher's job (`analyze_expr` covers them).
        let env = StatementEnv::default();
        let block = parse("RETURN { LET $x = 1; RETURN $x; };");
        let fact = infer_first(&block, "Block", None, &env);
        assert_eq!(fact.value_class, ExpressionValueClass::Block);
        assert!(!fact.partial.is_empty());
    }

    #[test]
    fn let_bound_objects_resolve_value_rooted_paths() {
        let mut env = StatementEnv::default();
        let mut fields = std::collections::BTreeMap::new();
        fields.insert("name".to_string(), Kind::String);
        env.define_let(
            "user".into(),
            ExpressionFact::new(
                SourceSpan::new(
                    SourceId::new("env"),
                    surrealguard_syntax::span::ByteRange::new(0, 1).unwrap(),
                ),
                ExpressionValueClass::Object,
            )
            .with_kind(Kind::Literal(surrealdb_types::KindLiteral::Object(fields))),
        );

        let parsed = parse("RETURN $user.name;");
        let fact = infer_first(&parsed, "Path", None, &env);

        assert_eq!(fact.kind, Some(Kind::String));
    }

    #[test]
    fn record_link_fields_step_through_the_schema() {
        let schema = schema_with_links();
        let table = schema.tables.get("person").unwrap();
        let env = StatementEnv::default();

        // best_friend is record<person>; stepping through it reaches the
        // target table's fields.
        let parsed = parse("SELECT best_friend.name FROM person;");
        let fact = infer_first_with_schema(&parsed, "Path", &schema, Some(table), &env);

        assert_eq!(fact.kind, Some(Kind::String));
    }

    #[test]
    fn method_calls_dispatch_by_receiver_kind() {
        let schema = person_schema();
        let table = schema.tables.get("person").unwrap();
        let env = StatementEnv::default();

        let parsed = parse("SELECT name.len() FROM person;");
        let fact = infer_first_with_schema(&parsed, "Path", &schema, Some(table), &env);

        assert_eq!(fact.kind, Some(Kind::Int));
    }

    #[test]
    fn generic_methods_resolve_on_any_receiver_kind() {
        // SurrealDB answers `.to_string()`, `.type_of()` and the `type::is_*`
        // predicates on every value, whatever its kind — including kinds with
        // no function family of their own (verified on 3.0.5). Without this
        // they were error-severity 5001 false positives on valid queries.
        let schema = person_schema();
        let table = schema.tables.get("person").unwrap();
        let env = StatementEnv::default();

        let cases = [
            ("SELECT age.to_string() FROM person;", Kind::String),
            ("SELECT name.type_of() FROM person;", Kind::String),
            ("SELECT age.is_none() FROM person;", Kind::Bool),
            ("SELECT name.is_number() FROM person;", Kind::Bool),
            // A receiver kind with no family of its own still answers them.
            ("SELECT (age > 1).to_string() FROM person;", Kind::String),
        ];
        for (query, expected) in cases {
            let parsed = parse(query);
            let fact = infer_first_with_schema(&parsed, "Path", &schema, Some(table), &env);
            assert_eq!(fact.kind, Some(expected), "query: {query}");
        }
    }

    #[test]
    fn a_method_the_receiver_does_not_have_stays_unresolved() {
        // The generic fallback must not turn every name into a resolved
        // method: `string` has no `.abs()` and no `.frobnicate()`, and
        // `int` has no `.len()` — all three are rejected by 3.0.5 too.
        let schema = person_schema();
        let table = schema.tables.get("person").unwrap();
        let env = StatementEnv::default();

        for query in [
            "SELECT name.frobnicate() FROM person;",
            "SELECT name.abs() FROM person;",
            "SELECT age.len() FROM person;",
        ] {
            let parsed = parse(query);
            let fact = infer_first_with_schema(&parsed, "Path", &schema, Some(table), &env);
            assert_eq!(fact.kind, None, "query: {query}");
        }
    }

    #[test]
    fn a_set_receiver_answers_the_two_array_methods_it_borrows() {
        // A `set` receiver dispatches `.every()`/`.includes()` (verified on
        // 3.0.5) but not the rest of `array`'s surface: `.sort()` and
        // `.push()` are rejected there.
        let parsed = parse_source(
            SourceId::new("infer:set-schema"),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD tags ON person TYPE set<string>;",
        )
        .expect("schema parses");
        let schema = extract_schema(&[parsed]).schema;
        let table = schema.tables.get("person").unwrap();
        let env = StatementEnv::default();

        let parsed = parse("SELECT tags.includes('a') FROM person;");
        let fact = infer_first_with_schema(&parsed, "Path", &schema, Some(table), &env);
        assert_eq!(fact.kind, Some(Kind::Bool));

        let parsed = parse("SELECT tags.len() FROM person;");
        let fact = infer_first_with_schema(&parsed, "Path", &schema, Some(table), &env);
        assert_eq!(fact.kind, Some(Kind::Int));

        let parsed = parse("SELECT tags.sort() FROM person;");
        let fact = infer_first_with_schema(&parsed, "Path", &schema, Some(table), &env);
        assert_eq!(fact.kind, None);
    }

    #[test]
    fn closure_taking_methods_resolve_and_derive_their_return_kind() {
        // Method sugar and the function form are the same call: `.map()` must
        // agree with `array::map()`. Dispatch used to build a synthetic call
        // with NO argument expressions, so `closure_arg` found nothing, every
        // closure-taking builtin fell back to `Kind::Any`, and `any` was then
        // read as "no such method" — an error-severity 5001 on valid input.
        let schema = person_schema();
        let table = schema.tables.get("person").unwrap();
        let env = StatementEnv::default();

        let cases = [
            // The closure body decides the element kind.
            (
                "SELECT [1, 2].map(|$v| $v > 0) FROM person;",
                Kind::Array(Box::new(Kind::Bool), Some(2)),
            ),
            // Fold/reduce take the closure's return kind directly.
            (
                "SELECT [1, 2].fold('', |$acc, $v| $acc) FROM person;",
                Kind::String,
            ),
            (
                "SELECT ['a'].reduce(|$acc, $v| $acc) FROM person;",
                Kind::String,
            ),
            // Filter keeps the element kind; the closure is a predicate.
            (
                "SELECT [1, 2].filter(|$v| $v > 0) FROM person;",
                Kind::Array(Box::new(Kind::Int), None),
            ),
            // `.chain()` has no builtin behind it; its kind is the closure's.
            ("SELECT [1, 2].chain(|$v| 'x') FROM person;", Kind::String),
            ("SELECT name.chain(|$v| 1) FROM person;", Kind::Int),
        ];
        for (query, expected) in cases {
            let parsed = parse(query);
            let fact = infer_first_with_schema(&parsed, "Path", &schema, Some(table), &env);
            assert_eq!(fact.kind, Some(expected), "query: {query}");
        }
    }

    #[test]
    fn a_method_whose_builtin_returns_any_still_resolves() {
        // `record::id` returns an honest `Kind::Any` (a record id is genuinely
        // one of many shapes). Conflating that with "no such method" made
        // `$r.id()` a false 5001. Resolution asks the builtin catalog; the
        // kind stays `any` rather than being invented.
        let schema = schema_with_links();
        let table = schema.tables.get("person").unwrap();
        let env = StatementEnv::default();

        let parsed = parse("SELECT best_friend.id() FROM person;");
        let fact = infer_first_with_schema(&parsed, "Path", &schema, Some(table), &env);
        assert_eq!(fact.kind, Some(Kind::Any));
    }

    #[test]
    fn every_builtin_in_a_receivers_family_resolves_as_a_method() {
        // The sweep contract: method sugar is `family::name(receiver, ...)`,
        // so EVERY name the analyzer dispatches in a receiver's family must
        // resolve as a method on it. This is derived from the builtin catalog
        // (itself proven arm-for-arm against the dispatch tables), so a
        // builtin added later is covered without editing this test.
        let families: &[(&str, Kind)] = &[
            ("array", Kind::Array(Box::new(Kind::Int), Some(2))),
            ("set", Kind::Set(Box::new(Kind::Int), Some(2))),
            ("string", Kind::String),
            ("object", Kind::Object),
            ("duration", Kind::Duration),
            ("time", Kind::Datetime),
            ("bytes", Kind::Bytes),
            ("record", Kind::Record(Vec::new())),
            ("math", Kind::Int),
        ];
        let mut checked = 0usize;
        crate::analyzer::test_support::with_ctx(|ctx| {
            for (family, receiver) in families {
                for builtin in crate::analyzer::function::builtin_catalog()
                    .iter()
                    .filter(|builtin| builtin.family() == *family)
                {
                    let method = builtin
                        .name
                        .strip_prefix(&format!("{family}::"))
                        .expect("family prefix");
                    // The nested `array::sort::asc` spelling is a path, not a
                    // method name; SurrealQL has no `.sort::asc()` sugar.
                    if method.contains("::") {
                        continue;
                    }
                    assert!(
                        method_result(receiver, method, std::slice::from_ref(receiver), &[], ctx)
                            .is_some(),
                        "`{}` does not resolve as `.{method}()` on a `{family}` receiver",
                        builtin.name,
                    );
                    checked += 1;
                }
            }
        });
        assert!(
            checked > 200,
            "expected the whole method surface, swept {checked}"
        );
    }

    #[test]
    fn an_unknown_method_name_still_fails_to_resolve() {
        // The counterpart to the sweep: existence is asked of the catalog, so
        // a name in no family and in no generic set stays unresolved (5001).
        crate::analyzer::test_support::with_ctx(|ctx| {
            for (receiver, method) in [
                (Kind::Array(Box::new(Kind::Int), None), "frobnicate"),
                (Kind::String, "map"),
                (Kind::Int, "filter"),
                (Kind::Object, "reduce"),
            ] {
                assert_eq!(
                    method_result(&receiver, method, std::slice::from_ref(&receiver), &[], ctx),
                    None,
                    "`.{method}()` must not resolve on a `{receiver}`",
                );
            }
        });
    }

    fn schema_with_links() -> SchemaIndex {
        let parsed = parse_source(
            SourceId::new("infer:schema-links"),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD best_friend ON person TYPE record<person>;",
        )
        .expect("schema parses");
        extract_schema(&[parsed]).schema
    }

    #[test]
    fn graph_idioms_are_explicit_partials_for_expression_inference() {
        let schema = person_schema();
        let table = schema.tables.get("person").unwrap();
        let env = StatementEnv::default();

        let parsed = parse("SELECT ->likes->post FROM person;");
        let fact = infer_first(&parsed, "Path", Some(table), &env);

        assert_eq!(fact.value_class, ExpressionValueClass::FieldPath);
        assert_eq!(fact.partial, vec![PartialReason::Unresolved]);
    }
}
