//! `SELECT` statement analysis: response-type inference over the typed AST.
//!
//! The inferred response type is a plain upstream `Kind`: closed objects
//! are `Kind::Literal(KindLiteral::Object(...))`, undeterminable positions
//! are `Kind::Any` poison values. Graph traversals, destructure selections,
//! and modifier clauses are consumed as structured `ast::*` values. The
//! only use of source text is *naming*, and only where SurrealDB itself
//! falls back to it: an unaliased projection is keyed by its *simplified*
//! form — a call by its bare function name (`string::len(name)` →
//! `string::len`), an idiom by its `Field`/`Graph` parts alone
//! (`name.len()` → `name`) — and by its own source text otherwise
//! (`SELECT age >= 18 FROM ...` produces the field `"age >= 18"`).

use std::collections::BTreeMap;

use surrealdb_types::{Kind, KindLiteral};
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::contract::{Contract, Position};
use crate::analyzer::expression::infer::{infer_expression_fact, plain_field_segments};
use crate::analyzer::facts::{Place, PlaceRoot};
use crate::schema::{SchemaIndex, TableDef};

pub(crate) fn analyze_select(ctx: &mut AnalysisContext<'_>, stmt: &ast::SelectStmt) -> Kind {
    select_response_kind(stmt, ctx)
}

/// Whether a function reads its argument's *cardinality* only — its emptiness
/// or its length — rather than any value inside it.
///
/// This is the consumer half of 4023's contract. In one of these positions an
/// ungrouped `SELECT count()` is correct and `GROUP ALL` is not an equivalent
/// spelling of it: engine-verified on 3.0.5, with no matching rows the
/// ungrouped form yields `[]` while the grouped form yields `[{count: 0}]`, so
/// `array::is_empty(…)` flips from true to false. Advising the grouped form
/// there does not improve the query; it inverts it.
pub(crate) fn reads_only_cardinality(path: &str) -> bool {
    matches!(
        path,
        "array::is_empty"
            | "array::is_not_empty"
            | "array::len"
            // `count(x)` counts what it is given; it never reads a total out
            // of it.
            | "count"
    )
}

/// Pure core: infers the response type of a lowered `SELECT`.
pub(crate) fn select_response_kind(stmt: &ast::SelectStmt, ctx: &mut AnalysisContext<'_>) -> Kind {
    // The marker names *this* SELECT's result. Everything nested inside its
    // clauses occupies its own position, so clear it before descending —
    // otherwise `array::is_empty(SELECT count() FROM t WHERE n = (SELECT
    // count() FROM u))` would silence the inner SELECT too, and that one is
    // read as a number.
    let cardinality_only = ctx.in_cardinality_position();
    ctx.with_cardinality_position(false, |ctx| {
        select_response_kind_inner(stmt, ctx, cardinality_only)
    })
}

fn select_response_kind_inner(
    stmt: &ast::SelectStmt,
    ctx: &mut AnalysisContext<'_>,
    cardinality_only: bool,
) -> Kind {
    check_select_statement_shape(stmt, ctx, cardinality_only);
    if stmt.explain.is_some() {
        return explain_response_kind();
    }
    let Some(from) = stmt.from.first() else {
        return Kind::Any;
    };
    let table_name = match resolve_from_table(stmt, from, ctx) {
        Ok(table_name) => table_name,
        Err(kind) => return kind,
    };
    let Some(table) = ctx.schema().tables.get(&table_name) else {
        crate::analyzer::data::check_table_reference(ctx, &table_name, from.span);
        return walk_projections_for_findings(stmt, ctx);
    };
    if let Some(kind) = check_source_table_shape(stmt, &table_name, table, from.span, ctx) {
        return kind;
    }

    check_where_clause(stmt, table, ctx);
    check_only_filter_cardinality(stmt, from, table, ctx);

    // Row-context clauses reference fields by name; each position has its
    // own code so hosts can configure them independently.
    for idiom in &stmt.omit {
        if let Some(segments) = plain_field_segments(&idiom.node) {
            check_clause_field_path(ctx, table, &segments, idiom.span);
        }
    }
    check_fetch_clauses(stmt, table, ctx);
    check_split_clauses(stmt, table, ctx);
    if let Some(group) = &stmt.group {
        // GROUP BY keys name *result* columns, so a projection alias is a
        // legal key even though the source table has no such field.
        let projected = projected_row_names(stmt);
        for idiom in &group.keys {
            if let Some(segments) = plain_field_segments(&idiom.node) {
                if projected_name_covers(&projected, &segments.join(".")) {
                    continue;
                }
                check_clause_field_path(ctx, table, &segments, idiom.span);
            }
        }
    }
    check_order_clause(stmt, table, ctx);

    let has_wildcard = has_wildcard_projection(stmt);

    // A wildcard contributes the whole materialized row — but only when the
    // rows are materialized. Under a GROUP clause the result rows are
    // synthesized from the *projection* (group keys and accumulators), and a
    // wildcard contributes nothing to that: SurrealDB 3.x rejects the query
    // outright ("expression `*` within in selector cannot be aggregated in a
    // group"), and 2.x builds its grouped output from the non-`*` fields
    // only, so `SELECT * FROM t GROUP BY k` yields rows with no keys at all.
    // Either way no declared field survives, so the wildcard is dropped and
    // the remaining projections alone type the row (`SELECT *, count() FROM t
    // GROUP BY k` is `{ count: number }`). Both behaviours verified against
    // the engine: 3.0.5 live, 2.x via `dbs::group::GroupsCollector`, which
    // iterates `fields.other()` and never sees `Field::All`. The rejection
    // itself is reported as 4025 at the `*`.
    //
    // The wildcard *seeds* the row; it does not replace the projection walk.
    // Sibling projections are layered on top of it — `SELECT *, ->has_account
    // AS acc FROM person` is every declared field plus `acc` (3.0.5 live).
    let row_kind = if has_wildcard && stmt.group.is_none() {
        projected_object_kind(stmt, &table_name, table, ctx, all_fields_map(table), true)
    } else if let Some(value_kind) = value_projection_kind(stmt, &table_name, table, ctx) {
        value_kind
    } else {
        projected_object_kind(stmt, &table_name, table, ctx, BTreeMap::new(), false)
    };
    // WHERE-narrowing (design §3.1): tighten each projected field the WHERE
    // clause provably constrains. Runs before OMIT/FETCH/SPLIT, which operate
    // on the same object-literal shape.
    let row_kind = apply_where_narrowing(row_kind, stmt);
    let row_kind = apply_omit(row_kind, &stmt.omit);
    let row_kind = apply_fetch(row_kind, &stmt.fetch, ctx.schema());
    let row_kind = apply_split(row_kind, &stmt.split);

    if stmt.only {
        // `FROM ONLY` is `option<row>`: the engine yields NONE (verified `IS
        // NONE` on 3.0.5, not NULL) whenever the target produces no row —
        // including a *concrete record id* that does not exist
        // (`SELECT * FROM ONLY account:ghost` → NONE) and a filter that
        // matches nothing. No `ONLY` form is statically guaranteed to produce
        // a row, so every one of them is optional; a caller that needs the
        // bare row narrows it with a guard (`IF $x = NONE THEN THROW … END`).
        Kind::either(vec![Kind::None, row_kind])
    } else {
        Kind::Array(Box::new(row_kind), literal_limit(stmt))
    }
}

/// Resolves the `FROM` source into the table whose schema types the rows.
/// `Ok(table_name)` continues with schema-typed inference; `Err(kind)` is a
/// fully-resolved response for sources that need no source table (subquery,
/// parameter, dynamic) or that cannot resolve to one.
fn resolve_from_table(
    stmt: &ast::SelectStmt,
    from: &ast::Spanned<ast::Expr>,
    ctx: &mut AnalysisContext<'_>,
) -> Result<String, Kind> {
    match &from.node {
        ast::Expr::Table(name) => Ok(name.node.clone()),
        ast::Expr::RecordId { table, .. } => Ok(table.node.clone()),
        ast::Expr::Idiom(idiom) => {
            if let Some(leading) = leading_field_table(idiom) {
                crate::analyzer::data::graph::check_graph_idiom_at(ctx, &leading, idiom, true);
            }
            match graph_source_table(idiom, ctx.schema()) {
                Some(table) => Ok(table),
                None => Err(Kind::Any),
            }
        }
        // A subquery source iterates the inner response's rows: with a
        // wildcard projection the row type is the inner element type.
        // (Field projections over subquery rows need object-literal field
        // lookup against the inner element — not built.)
        ast::Expr::Subquery(inner) => {
            let all_wildcards = stmt
                .projections
                .iter()
                .all(|projection| matches!(projection, ast::Projection::Wildcard(_)));
            if !all_wildcards {
                return Err(Kind::Any);
            }
            let inner_kind = ctx.with_row_table(None, |ctx| {
                crate::analyzer::expression::infer::statement_value_kind(inner, ctx)
            });
            let row_kind = match inner_kind {
                Some(Kind::Array(element, _)) => *element,
                Some(other) => other,
                None => return Err(Kind::Any),
            };
            Err(if stmt.only {
                // Same optionality as a table source: an inner query that
                // produces no rows makes the `ONLY` result NONE (verified
                // `IS NONE` on 3.0.5).
                Kind::either(vec![Kind::None, row_kind])
            } else {
                Kind::Array(Box::new(row_kind), literal_limit(stmt))
            })
        }
        // A parameter source. When it is already bound to a concrete
        // `record<T>` (e.g. a `record<T>` function parameter, or a narrowed
        // `$x.parent`), that IS the source table — project against it like a
        // plain table. `record<a|b>` unions and unbound params fall through to
        // the host-param path below.
        ast::Expr::Param(param) => {
            if let Some(Kind::Record(tables)) =
                ctx.env().let_fact(param).and_then(|fact| fact.kind.clone())
            {
                if let [table] = tables.as_slice() {
                    // Only a DEFINED table is a usable source. A dangling
                    // `record<undefined>` is already flagged at its declaration
                    // (E1001) — don't re-report it here; fall through to `Any`.
                    if ctx.schema().tables.contains_key(&table.to_string()) {
                        return Ok(table.to_string());
                    }
                }
            }
            let tables: Vec<surrealdb_types::Value> = ctx
                .schema()
                .tables
                .keys()
                .map(|name| surrealdb_types::Value::String(name.clone()))
                .collect();
            let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), from.span);
            ctx.constrain_param(
                param,
                span,
                Kind::Any,
                (!tables.is_empty()).then_some(crate::analysis::ValueDomain::OneOf(tables)),
            );
            Err(walk_projections_for_findings(stmt, ctx))
        }
        // Dynamic sources and anything else stay undetermined.
        _ => Err(walk_projections_for_findings(stmt, ctx)),
    }
}

/// Schema-shape gates on the resolved source: DROP tables never retain rows
/// (4022, non-fatal) and a fieldless table (with no graph projection to type)
/// limits analysis (7008). `Some(kind)` short-circuits to a projection-only
/// walk; `None` continues with schema-typed inference.
fn check_source_table_shape(
    stmt: &ast::SelectStmt,
    table_name: &str,
    table: &TableDef,
    from_span: surrealguard_syntax::span::ByteRange,
    ctx: &mut AnalysisContext<'_>,
) -> Option<Kind> {
    if table.drop_table {
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), from_span);
        ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                span,
                4022,
                format!("`{table_name}` is a DROP table, so this SELECT never returns rows"),
            )
            .with_help("DROP tables discard every row on write"),
        );
    }
    if table.fields.is_empty() && !stmt.projections.iter().any(is_graph_projection) {
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), from_span);
        ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                span,
                7008,
                format!("`{table_name}` has no declared fields, so field-level checks are skipped"),
            )
            .with_help(format!(
                "add `DEFINE FIELD` declarations to `{table_name}` for full analysis"
            )),
        );
        return Some(walk_projections_for_findings(stmt, ctx));
    }
    None
}

/// The WHERE clause: findings are emitted inside the condition (with row
/// fields resolvable), and the condition contract (2005) flags a kind that
/// can never be truthy-tested. The condition's own kind never affects the
/// response.
fn check_where_clause<'a>(
    stmt: &ast::SelectStmt,
    table: &'a TableDef,
    ctx: &mut AnalysisContext<'a>,
) {
    let Some(cond) = &stmt.where_clause else {
        return;
    };
    // The WHERE kind is irrelevant to the response; the walk emits
    // findings inside the condition, with row fields resolvable.
    let cond_kind = ctx.with_row_table(Some(table), |ctx| {
        let fact = infer_expression_fact(cond, ctx);
        crate::analyzer::expression::check::check_value_expression(ctx, cond);
        fact.kind
    });
    crate::analyzer::data::check_expression_field_paths(ctx, table, cond, 1002);
    if let Some(kind) = cond_kind {
        if Contract::condition(Position::WhereSelect)
            .decide(&kind)
            .is_violation()
        {
            let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), cond.span);
            ctx.emit(
                surrealguard_diagnostics::catalog::finding(
                    span,
                    2005,
                    format!(
                        "this WHERE condition is a `{}`, not a `bool`",
                        crate::render::render_offending(&kind, Some(&Kind::Bool))
                    ),
                )
                .with_help(
                    "a WHERE filter keeps rows where the condition is true; it must be a bool",
                ),
            );
        }
    }
}

/// FETCH clauses: bare field names and aliased projections must name
/// something that can hold records — otherwise the FETCH does nothing
/// (1023). Field paths are also checked against the schema (1002).
fn check_fetch_clauses(stmt: &ast::SelectStmt, table: &TableDef, ctx: &mut AnalysisContext<'_>) {
    for idiom in &stmt.fetch {
        // FETCH also accepts projection aliases; only bare field names are
        // checkable here.
        let named_alias = stmt.projections.iter().any(|projection| {
            matches!(projection, ast::Projection::Expr { alias: Some(alias), .. }
                if idiom.node.parts.len() == 1
                    && matches!(&idiom.node.parts[0].node, ast::IdiomPart::Field(name) if *name == alias.node))
        });
        if named_alias {
            // The alias must still name something that can hold records.
            if idiom.node.parts.len() == 1 {
                if let ast::IdiomPart::Field(alias_name) = &idiom.node.parts[0].node {
                    let aliased_kind =
                        stmt.projections
                            .iter()
                            .find_map(|projection| match projection {
                                ast::Projection::Expr {
                                    expr,
                                    alias: Some(alias),
                                } if alias.node == *alias_name => ctx
                                    .with_row_table(ctx.schema().tables.get(&table.name), |ctx| {
                                        infer_expression_fact(expr, ctx).kind
                                    }),
                                _ => None,
                            });
                    if let Some(kind) = aliased_kind {
                        if kind != Kind::Any && !kind_may_hold_record(&kind) {
                            let span = surrealguard_syntax::span::SourceSpan::new(
                                ctx.source().clone(),
                                idiom.span,
                            );
                            ctx.emit(
                                surrealguard_diagnostics::catalog::finding(
                                    span,
                                    1023,
                                    format!(
                                        "FETCH `{alias_name}` does nothing — `{}` holds no records",
                                        crate::render::render_offending(
                                            &kind,
                                            Some(&Kind::Record(Vec::new()))
                                        )
                                    ),
                                )
                                .with_help("FETCH only expands record links, not scalar values"),
                            );
                        }
                    }
                }
            }
            continue;
        }
        if let Some(segments) = plain_field_segments(&idiom.node) {
            if check_clause_field_path(ctx, table, &segments, idiom.span) {
                continue;
            }
            // FETCH substitutes records; fetching a scalar does nothing.
            // Resolve across record links so `FETCH team.owner` reads the
            // linked field's kind rather than the opaque `Any` boundary.
            if let Some(kind) = resolve_field_path(ctx.schema(), table, &segments) {
                if kind != Kind::Any && !kind_may_hold_record(&kind) {
                    let span = surrealguard_syntax::span::SourceSpan::new(
                        ctx.source().clone(),
                        idiom.span,
                    );
                    ctx.emit(
                        surrealguard_diagnostics::catalog::finding(
                            span,
                            1023,
                            format!(
                                "FETCH `{}` does nothing — `{}` holds no records",
                                segments.join("."),
                                crate::render_kind(&kind)
                            ),
                        )
                        .with_help("FETCH only expands record links, not scalar values"),
                    );
                }
            }
        }
    }
}

/// SPLIT clauses: each key must name a collection field to fan rows out over
/// (1024). Field paths are also checked against the schema (1002).
fn check_split_clauses(stmt: &ast::SelectStmt, table: &TableDef, ctx: &mut AnalysisContext<'_>) {
    for idiom in &stmt.split {
        if let Some(segments) = plain_field_segments(&idiom.node) {
            if check_clause_field_path(ctx, table, &segments, idiom.span) {
                continue;
            }
            // SPLIT fans rows out over a collection field. Resolve across
            // record links so a linked collection field types precisely.
            if let Some(kind) = resolve_field_path(ctx.schema(), table, &segments) {
                if Contract::possible(Position::Split, collection_kind())
                    .decide(&kind)
                    .is_violation()
                {
                    let span = surrealguard_syntax::span::SourceSpan::new(
                        ctx.source().clone(),
                        idiom.span,
                    );
                    ctx.emit(
                        surrealguard_diagnostics::catalog::finding(
                            span,
                            1024,
                            format!(
                                "SPLIT needs a collection field, but `{}` is a `{}`",
                                segments.join("."),
                                crate::render_kind(&kind)
                            ),
                        )
                        .with_help(
                            "SPLIT fans one output row out per array/set element; a scalar has nothing to split",
                        ),
                    );
                }
            }
        }
    }
}

/// ORDER BY's contract (2017): each key names a field available on the
/// result rows — a field of the source (checked against the schema), and,
/// when the projection list is explicit, one of the projected names.
/// SurrealDB's own parser enforces both; our grammar is more permissive, so
/// the contract is enforced here. `ORDER BY RAND()` is the one non-field form.
fn check_order_clause(stmt: &ast::SelectStmt, table: &TableDef, ctx: &mut AnalysisContext<'_>) {
    let Some(order) = &stmt.order else {
        return;
    };
    let explicit_keys: Option<Vec<String>> = if stmt
        .projections
        .iter()
        .any(|projection| matches!(projection, ast::Projection::Wildcard(_)))
    {
        None
    } else {
        Some(
            stmt.projections
                .iter()
                .filter_map(|projection| match projection {
                    ast::Projection::Expr { expr, alias } => Some(match alias {
                        Some(alias) => alias.node.clone(),
                        None => slice(ctx.source_text(), expr.span).to_string(),
                    }),
                    _ => None,
                })
                .collect(),
        )
    };
    // ORDER BY keys, like GROUP BY keys, name *result* columns: an alias is
    // a legal key even though the source table has no such field.
    let projected = projected_row_names(stmt);
    for key in &order.keys {
        let field = match &key.expr.node {
            ast::Expr::Idiom(idiom) => plain_field_segments(idiom),
            // ORDER BY RAND() is the one non-field form.
            ast::Expr::Call(call) if call.path.node == "rand" => continue,
            _ => None,
        };
        let Some(segments) = field else {
            let span =
                surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), key.expr.span);
            ctx.emit(surrealguard_diagnostics::catalog::finding(
                span,
                2017,
                "ORDER BY must name a field of the result rows (or `RAND()`)".to_string(),
            ));
            continue;
        };
        // A key that names nothing at all is reported as 1002 and nothing
        // else: 2017's remedy — project the key — does not fix a field the
        // table does not have, so offering it would send the author the wrong
        // way about the same single defect.
        if !projected_name_covers(&projected, &segments.join("."))
            && check_clause_field_path(ctx, table, &segments, key.expr.span)
        {
            continue;
        }
        if let Some(keys) = &explicit_keys {
            let name = segments.join(".");
            if !keys.contains(&name) {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), key.expr.span);
                let mut finding = surrealguard_diagnostics::catalog::finding(
                    span,
                    2017,
                    format!("ORDER BY `{name}` doesn't name a field of this query's rows"),
                );
                if let Some(def) = table.fields.get(&name) {
                    finding = finding
                        .with_related(def.name_span.clone(), format!("`{name}` is defined here"));
                }
                ctx.emit(finding);
            }
        }
    }
}

/// Walks projection expressions for their findings when the row type
/// cannot be resolved (unknown/schemaless/dynamic sources) — misuse inside
/// a projection is real regardless of the table.
fn walk_projections_for_findings(stmt: &ast::SelectStmt, ctx: &mut AnalysisContext<'_>) -> Kind {
    for projection in &stmt.projections {
        if let ast::Projection::Expr { expr, .. } = projection {
            infer_expression_fact(expr, ctx);
            crate::analyzer::expression::check::check_value_expression(ctx, expr);
        }
    }
    if let Some(cond) = &stmt.where_clause {
        infer_expression_fact(cond, ctx);
        crate::analyzer::expression::check::check_value_expression(ctx, cond);
    }
    Kind::Any
}

/// One row-count clause value — `LIMIT`/`START`, wherever it is written. The
/// contract does not change with the position: it takes a non-negative integer
/// (2018), and an unbound param in the slot is constrained to one.
///
/// A graph step spells the same two clauses inside its parentheses
/// (`->(likes LIMIT 3)`), and the step is not a place where `LIMIT -1` becomes
/// acceptable — so it calls this rather than growing a second opinion.
pub(crate) fn check_row_count_clause(
    ctx: &mut AnalysisContext<'_>,
    expr: &ast::Spanned<ast::Expr>,
    name: &str,
    position: Position,
) {
    if let ast::Expr::Param(param) = &expr.node {
        if ctx.env().let_fact(param).is_none() {
            let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), expr.span);
            ctx.constrain_param(
                param,
                span,
                Kind::Int,
                Some(crate::analysis::ValueDomain::Range {
                    min: Some(0),
                    max: None,
                }),
            );
            return;
        }
    }
    let fact = infer_expression_fact(expr, ctx);
    if let Some(kind) = &fact.kind {
        // `number` is admitted alongside `int` because that is what an
        // arithmetic or `math::` result infers as, and the clause takes it.
        if Contract::possible(position, Kind::either(vec![Kind::Int, Kind::Number]))
            .decide(kind)
            .is_violation()
        {
            let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), expr.span);
            ctx.emit(surrealguard_diagnostics::catalog::finding(
                span,
                2018,
                format!(
                    "{name} needs an integer, but this is a `{}`",
                    crate::render::render_offending(kind, Some(&Kind::Int))
                ),
            ));
            return;
        }
    }
    if let Some(surrealdb_types::Value::Number(surrealdb_types::Number::Int(value))) = &fact.value {
        if *value < 0 {
            let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), expr.span);
            ctx.emit(surrealguard_diagnostics::catalog::finding(
                span,
                2018,
                format!("{name} can't be negative"),
            ));
        }
    }
}

/// Clause-value invariants: LIMIT/START must be integers (2018) and
/// non-negative when constant (2024); TIMEOUT takes a duration (2019).
fn check_clause_values(stmt: &ast::SelectStmt, ctx: &mut AnalysisContext<'_>) {
    for (clause, name, position) in [
        (&stmt.limit, "LIMIT", Position::Limit),
        (&stmt.start, "START", Position::Start),
    ] {
        if let Some(expr) = clause {
            check_row_count_clause(ctx, expr, name, position);
        }
    }
    if let Some(expr) = &stmt.timeout {
        let kind = infer_expression_fact(expr, ctx).kind;
        if let Some(kind) = kind {
            if Contract::possible(Position::Timeout, Kind::Duration)
                .decide(&kind)
                .is_violation()
            {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), expr.span);
                ctx.emit(surrealguard_diagnostics::catalog::finding(
                    span,
                    2019,
                    format!(
                        "TIMEOUT needs a duration, but this is a `{}`",
                        crate::render::render_offending(&kind, Some(&Kind::Duration))
                    ),
                ));
            }
        }
    }
}

/// 4026 — a **filtered** `FROM ONLY` whose target is not provably single-row.
///
/// Verified on a live 3.0.5: `SELECT * FROM ONLY t WHERE <2 matches>` fails
/// with `Expected a single result output when using the ONLY keyword`, while
/// the same query succeeds unchanged when the filter happens to match one row
/// (and yields NONE when it matches none). The failure is therefore a property
/// of the *data*, not of the query — hence a warning, not an error: a filter
/// can be single-row for reasons the schema does not state (a convention, an
/// application invariant), and rejecting those outright would fail working
/// programs. What the finding reports is an *unproven* guarantee.
///
/// It stays silent wherever at most one row is provable — each case verified
/// against 3.0.5:
///
/// * a record-id target (`FROM ONLY account:x WHERE …`) — one row by
///   construction; a record-id *range* (`account:a..z`) is not,
/// * `WHERE id = …` — `id` is the primary key,
/// * an equality covering **every** field of a `UNIQUE` index (a partial
///   cover does not: `UNIQUE (org, usr)` with only `org = 'o1'` still errors),
/// * an explicit `LIMIT 1` (`LIMIT 2` still errors).
///
/// Only the `AND` conjunction contributes: `WHERE email = 'a' OR email = 'b'`
/// on a `UNIQUE email` errors on two matches, so a disjunction proves nothing.
///
/// The *unfiltered* table-wide case belongs to 4003, which fires there instead.
fn check_only_filter_cardinality(
    stmt: &ast::SelectStmt,
    from: &ast::Spanned<ast::Expr>,
    table: &TableDef,
    ctx: &mut AnalysisContext<'_>,
) {
    if !stmt.only {
        return;
    }
    // 4003 owns the unfiltered target; this code is the filtered sibling.
    let Some(cond) = &stmt.where_clause else {
        return;
    };
    if only_filter_is_single_row(stmt, from, table, &cond.node) {
        return;
    }
    let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), cond.span);
    ctx.emit(
        surrealguard_diagnostics::catalog::finding(
            span,
            4026,
            "this filter isn't provably single-row, so `ONLY` fails at runtime \
             as soon as two rows match"
                .to_string(),
        )
        .with_help(
            "add `LIMIT 1`, filter on `id` or on every field of a UNIQUE index, \
             or drop `ONLY` and take the first row",
        ),
    );
}

/// Whether a filtered `FROM ONLY` provably yields at most one row. See
/// [`check_only_filter_cardinality`] for the case-by-case justification.
fn only_filter_is_single_row(
    stmt: &ast::SelectStmt,
    from: &ast::Spanned<ast::Expr>,
    table: &TableDef,
    cond: &ast::Expr,
) -> bool {
    if literal_limit(stmt).is_some_and(|limit| limit <= 1) {
        return true;
    }
    if matches!(&from.node, ast::Expr::RecordId { range: false, .. }) {
        return true;
    }
    let mut pinned = Vec::new();
    collect_equality_pinned_fields(cond, &mut pinned);
    if pinned.iter().any(|path| path == "id") {
        return true;
    }
    table.indexes.values().any(|index| {
        index.kind == crate::schema::IndexKind::Unique && {
            let covered = index.field_paths();
            !covered.is_empty() && covered.iter().all(|path| pinned.contains(path))
        }
    })
}

/// The dotted row-field paths a `WHERE` clause pins with an equality that
/// holds for *every* surviving row: the `AND` conjunction of `field = <expr>`
/// leaves, in either operand order. The compared value's shape does not
/// matter — a `UNIQUE` index bounds the match count whatever it is compared
/// against — so a param or an idiom on the other side counts just as a
/// literal does. `OR` contributes nothing.
fn collect_equality_pinned_fields(cond: &ast::Expr, out: &mut Vec<String>) {
    let ast::Expr::Binary { lhs, op, rhs } = cond else {
        return;
    };
    match &op.node {
        ast::BinaryOp::And => {
            collect_equality_pinned_fields(&lhs.node, out);
            collect_equality_pinned_fields(&rhs.node, out);
        }
        ast::BinaryOp::Eq | ast::BinaryOp::Exact | ast::BinaryOp::Is => {
            for (field, other) in [(&lhs.node, &rhs.node), (&rhs.node, &lhs.node)] {
                let ast::Expr::Idiom(idiom) = field else {
                    continue;
                };
                let Some(segments) = plain_field_segments(idiom) else {
                    continue;
                };
                // Field-vs-field (`email = name`) pins nothing: the compared
                // value varies per row, so a UNIQUE index on `email` bounds
                // nothing. Only a row-independent right-hand side pins.
                if is_row_independent(other) {
                    out.push(segments.join("."));
                }
            }
        }
        _ => {}
    }
}

/// Whether an expression's value is the same for every row of the scanned
/// table — it reads no bare row field. Only then does `field = <expr>` pin
/// `field` to a single value across the whole scan (so that a `UNIQUE` index
/// or the `id` key bounds the match count). Conservative in the direction of
/// *suppressing* 4026: an unrecognized shape is treated as row-dependent.
fn is_row_independent(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Literal(_) | ast::Expr::Param(_) | ast::Expr::RecordId { .. } => true,
        ast::Expr::Cast { expr, .. } | ast::Expr::Prefix { expr, .. } => {
            is_row_independent(&expr.node)
        }
        ast::Expr::Array(items) => items.iter().all(|item| is_row_independent(&item.node)),
        ast::Expr::Object(entries) => entries
            .iter()
            .all(|(_, value)| is_row_independent(&value.node)),
        ast::Expr::Call(call) => call.args.iter().all(|arg| is_row_independent(&arg.node)),
        ast::Expr::Binary { lhs, rhs, .. } => {
            is_row_independent(&lhs.node) && is_row_independent(&rhs.node)
        }
        // `$param.path` roots in a value, not in the row; a bare `field.path`
        // roots in the row.
        ast::Expr::Idiom(idiom) => match idiom.parts.first().map(|part| &part.node) {
            Some(ast::IdiomPart::Start(start)) => is_row_independent(&start.node),
            _ => false,
        },
        _ => false,
    }
}

/// Statement-shape invariants that don't depend on the schema: ONLY
/// without a single-row guarantee (4003 — a deterministic
/// `SingleOnlyOutput` runtime error) and duplicate projection keys (4011).
/// (VALUE's single-projection rule needs no finding: both SurrealDB's
/// parser and ours reject the syntax.)
fn check_select_statement_shape(
    stmt: &ast::SelectStmt,
    ctx: &mut AnalysisContext<'_>,
    cardinality_only: bool,
) {
    check_clause_values(stmt, ctx);
    check_count_without_group(stmt, ctx, cardinality_only);
    check_wildcard_under_group(stmt, ctx);
    check_group_key_projection(stmt, ctx);
    check_non_key_projection_under_group(stmt, ctx);
    check_page_without_order(stmt, ctx);

    // `SELECT *, age` — the explicit field is already inside `*`.
    if has_wildcard_projection(stmt) {
        for projection in &stmt.projections {
            if let ast::Projection::Expr { expr, alias: None } = projection {
                if let ast::Expr::Idiom(idiom) = &expr.node {
                    if plain_field_segments(idiom).is_some() {
                        let span = surrealguard_syntax::span::SourceSpan::new(
                            ctx.source().clone(),
                            expr.span,
                        );
                        ctx.emit(
                            surrealguard_diagnostics::catalog::finding(
                                span,
                                7007,
                                "this field is already included by `*`".to_string(),
                            )
                            .with_help("remove the explicit field, or drop the `*`"),
                        );
                    }
                }
            }
        }
    }

    // 7015 (opt-in, off by default): a plain `SELECT *` over-fetches every
    // column and makes result shapes brittle to schema drift. Distinct from
    // 7007, which fires only on the redundant `SELECT *, field` overlap — so
    // this fires only when the wildcard is the *sole* projection.
    if stmt.projections.len() == 1 {
        if let Some(ast::Projection::Wildcard(range)) = stmt.projections.first() {
            let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), *range);
            ctx.emit(
                surrealguard_diagnostics::catalog::finding(
                    span,
                    7015,
                    "`SELECT *` fetches every column and breaks silently when the schema changes"
                        .to_string(),
                )
                .with_help(
                    "project the fields you need, or allow this with `7015 = \"allow\"` (it is off by default)",
                ),
            );
        }
    }

    // 7014 (opt-in, off by default): a whole-table read with neither WHERE
    // nor LIMIT scans every row — the read-side analogue of 7009. ONLY on a
    // bare table is already a 4003 error, so it is excluded here.
    if !stmt.only && stmt.where_clause.is_none() && stmt.limit.is_none() {
        if let Some(from) = stmt.from.first() {
            if let ast::Expr::Table(name) = &from.node {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), from.span);
                ctx.emit(
                    surrealguard_diagnostics::catalog::finding(
                        span,
                        7014,
                        format!(
                            "`SELECT … FROM {0}` reads the whole `{0}` table",
                            name.node
                        ),
                    )
                    .with_help(
                        "add a `WHERE`/`LIMIT`, or allow this with `7014 = \"allow\"` (it is off by default)",
                    ),
                );
            }
        }
    }

    // Reads that hide writes: a mutation used as a projection or filter.
    for projection in &stmt.projections {
        if let ast::Projection::Expr { expr, .. } = projection {
            check_read_position_subquery(ctx, expr);
        }
    }
    if let Some(cond) = &stmt.where_clause {
        check_read_position_subquery(ctx, cond);
    }
    if stmt.only {
        let table_target = stmt
            .from
            .first()
            .filter(|from| matches!(from.node, ast::Expr::Table(_)));
        let limited_to_one = literal_limit(stmt).is_some_and(|limit| limit <= 1);
        // A WHERE clause enforces single-row cardinality at runtime (a
        // filtered `FROM ONLY <table>` selects the matching record), so it
        // needs no explicit LIMIT 1.
        if let Some(from) = table_target {
            if !limited_to_one && stmt.where_clause.is_none() {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), from.span);
                ctx.emit(
                    surrealguard_diagnostics::catalog::finding(
                        span,
                        4003,
                        "ONLY needs a single-row target, but this reads a whole table".to_string(),
                    )
                    .with_help("add `LIMIT 1`, a `WHERE`, or target a record id"),
                );
            }
        }
    }

    let mut seen = std::collections::BTreeMap::new();
    for projection in &stmt.projections {
        let ast::Projection::Expr { expr, alias } = projection else {
            continue;
        };
        let key = match alias {
            Some(alias) => alias.node.clone(),
            None => slice(ctx.source_text(), expr.span).to_string(),
        };
        let span = alias.as_ref().map_or(expr.span, |a| a.span);
        if seen.insert(key.clone(), span).is_some() {
            let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), span);
            ctx.emit(
                surrealguard_diagnostics::catalog::finding(
                    span,
                    4011,
                    format!("`{key}` is projected twice; the later one wins"),
                )
                .with_help("rename one projection with `AS <alias>`"),
            );
        }
    }
}

/// 4025: SurrealDB 3.x rejects a wildcard projection under *any* GROUP
/// clause outright — "Incorrect selector for aggregate selection, expression
/// `*` within in selector cannot be aggregated in a group". Verified on a
/// live 3.0.5 for `GROUP BY k`, `GROUP ALL`, and the mixed
/// `SELECT *, count() … GROUP BY k` / `… GROUP ALL` forms, including over a
/// table that doesn't exist (so it is a query-shape rejection, not a
/// row-dependent one). 2.x doesn't reject it but silently drops the `*`,
/// building grouped rows from the non-`*` fields only — so under either
/// engine the query never returns what its author asked for, which is why
/// this is an error rather than a version-gated warning.
///
/// `t.*` in expression position (`SELECT person.* … GROUP ALL`) is a
/// different construct — an idiom with an `All` part, which 3.0.5 accepts —
/// and is not a `Projection::Wildcard`, so it never reaches this.
fn check_wildcard_under_group(stmt: &ast::SelectStmt, ctx: &mut AnalysisContext<'_>) {
    if stmt.group.is_none() {
        return;
    }
    for projection in &stmt.projections {
        let ast::Projection::Wildcard(range) = projection else {
            continue;
        };
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), *range);
        ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                span,
                4025,
                "`*` cannot be aggregated by a GROUP clause — SurrealDB rejects this query"
                    .to_string(),
            )
            .with_help(
                "replace `*` with the group keys and aggregates you want, e.g. `SELECT status, count() FROM t GROUP BY status`",
            ),
        );
    }
}

/// Whether the statement projects a wildcard — the shape 4025 reports under
/// a GROUP clause, and the one that makes 4013 redundant there.
fn has_wildcard_projection(stmt: &ast::SelectStmt) -> bool {
    stmt.projections
        .iter()
        .any(|projection| matches!(projection, ast::Projection::Wildcard(_)))
}

/// 4013: a `GROUP BY` key that is not among the projected columns cannot
/// appear in the result rows — the grouping label is silently dropped, so the
/// rows can't be told apart. SurrealDB runs the query (it does not reject
/// this), which is why it is a warning rather than an error. Conservative to
/// zero false positives: suppressed when an unparseable projection is present
/// (the key may be covered by it), for `SELECT VALUE` (a single value
/// projection carries no named keys), and for `GROUP ALL`. A wildcard
/// projection also suppresses it — not because `*` covers the key (it covers
/// nothing under a GROUP clause) but because that query is *rejected*, which
/// 4025 reports as an error at the `*` itself; 4013's premise, that the query
/// runs and merely returns unlabelled rows, doesn't hold there. A key counts
/// as projected when its dotted path equals — or is a prefix of — a projected
/// field path or alias (projecting `address` covers a `GROUP BY address.city`).
fn check_group_key_projection(stmt: &ast::SelectStmt, ctx: &mut AnalysisContext<'_>) {
    let Some(group) = &stmt.group else {
        return;
    };
    if group.all || group.keys.is_empty() || stmt.value || has_wildcard_projection(stmt) {
        return;
    }
    if stmt
        .projections
        .iter()
        .any(|projection| matches!(projection, ast::Projection::Partial(_)))
    {
        return;
    }
    let projected = projected_row_names(stmt);
    // A key that names nothing on the source table is 1002's to report, and
    // only 1002's: this warning's remedy is "add the key to the projection",
    // which cannot label a group by a field that does not exist. The lookup is
    // the plain, side-effect-free one because the statement's own resolution
    // (which emits) has not run yet at shape-check time.
    let absent: std::collections::BTreeSet<String> = match plain_source_table(stmt, ctx.schema()) {
        Some(table) => group
            .keys
            .iter()
            .filter_map(|key| plain_field_segments(&key.node))
            .filter(|segments| field_path_is_absent(ctx.schema(), table, segments))
            .map(|segments| segments.join("."))
            .collect(),
        None => std::collections::BTreeSet::new(),
    };
    for key in &group.keys {
        let Some(segments) = plain_field_segments(&key.node) else {
            continue;
        };
        let name = segments.join(".");
        if absent.contains(&name) {
            continue;
        }
        if !projected_name_covers(&projected, &name) {
            let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), key.span);
            ctx.emit(
                surrealguard_diagnostics::catalog::finding(
                    span,
                    4013,
                    format!(
                        "GROUP BY `{name}` is not projected, so it can't appear in the result rows"
                    ),
                )
                .with_help(format!(
                    "add `{name}` to the projection so each group is labelled by its key"
                )),
            );
        }
    }
}

/// 4029: under `GROUP BY k` every projection must be a group key, an
/// aggregate over a column, or an expression built from those. Anything else
/// — a plain non-key field, an expression over one — is not rejected by the
/// engine: each group silently *accumulates* every row's value into an array,
/// so the result shape is not what the projection reads as (`name: string`
/// comes back as `name: array<string>`). The inference side of the same
/// contract is [`group_accumulates`], which wraps exactly the projections
/// this reports.
///
/// Sibling of 4013 (a key that is not projected) and 4025 (a wildcard, which
/// the engine rejects outright — so this stays silent alongside one, as 4013
/// does: the premise that the query runs does not hold there). `GROUP ALL`
/// has no keys and every non-aggregate projection accumulates by
/// construction, but that is what the author asked for, so it is exempt.
fn check_non_key_projection_under_group(stmt: &ast::SelectStmt, ctx: &mut AnalysisContext<'_>) {
    let Some(group) = &stmt.group else {
        return;
    };
    if group.all || group.keys.is_empty() || has_wildcard_projection(stmt) {
        return;
    }
    for projection in &stmt.projections {
        let ast::Projection::Expr { expr, alias } = projection else {
            continue;
        };
        let Some(offender) = accumulated_field(expr, alias.as_ref(), group) else {
            continue;
        };
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), expr.span);
        ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                span,
                4029,
                format!(
                    "`{offender}` is neither a GROUP BY key nor an aggregate, so each group collects every row's value into an array"
                ),
            )
            .with_help(format!(
                "add `{offender}` to `GROUP BY`, or aggregate it (e.g. `array::group({offender})`) if you want each group's values"
            )),
        );
    }
}

/// Whether a `GROUP BY` clause accumulates this projection into an array
/// rather than reducing it — the predicate 4029 reports and inference wraps,
/// shared so the finding and the type can never disagree. `None` for
/// `GROUP ALL`, for a wildcard query (rejected; 4025), and for every
/// projection that is provably fine or not provably wrong.
fn group_accumulates(
    stmt: &ast::SelectStmt,
    expr: &ast::Spanned<ast::Expr>,
    alias: Option<&ast::Spanned<String>>,
) -> bool {
    let Some(group) = &stmt.group else {
        return false;
    };
    if group.all || group.keys.is_empty() || has_wildcard_projection(stmt) {
        return false;
    }
    accumulated_field(expr, alias, group).is_some()
}

/// The first row field that makes a projection accumulate under `GROUP BY`,
/// or `None` when the projection is a key, an aggregate, an expression built
/// from those, or a shape this does not model (then it stays silent).
///
/// A projection is a key when its alias is one, or when its plain field path
/// equals a key, covers one (`address` covers `GROUP BY address.city`), or
/// sits under one (`address.city` under `GROUP BY address` is constant within
/// a group). Everything the walk does not recognise — a subquery, a graph
/// traversal, a method call, a parameter-rooted idiom, a key that is not a
/// plain path — is treated as unknown, and one unknown anywhere silences the
/// whole projection.
fn accumulated_field(
    expr: &ast::Spanned<ast::Expr>,
    alias: Option<&ast::Spanned<String>>,
    group: &ast::GroupClause,
) -> Option<String> {
    let mut keys = Vec::with_capacity(group.keys.len());
    for key in &group.keys {
        keys.push(plain_field_segments(&key.node)?.join("."));
    }
    if alias.is_some_and(|alias| keys.contains(&alias.node)) {
        return None;
    }
    let mut offender = None;
    if field_walk_is_modeled(&expr.node, &keys, &mut offender) {
        offender
    } else {
        None
    }
}

/// Walks an expression looking for a row field that is not covered by the
/// group keys, outside any aggregate call. Returns `false` the moment it
/// meets a construct it does not model; `offender` receives the first
/// uncovered field's dotted path.
fn field_walk_is_modeled(expr: &ast::Expr, keys: &[String], offender: &mut Option<String>) -> bool {
    match expr {
        ast::Expr::Idiom(idiom) => {
            let Some(segments) = plain_field_segments(idiom) else {
                return false;
            };
            let path = segments.join(".");
            let covered = keys.iter().any(|key| {
                *key == path
                    || key.starts_with(&format!("{path}."))
                    || path.starts_with(&format!("{key}."))
            });
            if !covered && offender.is_none() {
                *offender = Some(path);
            }
            true
        }
        // An aggregate reduces whatever column it is handed; its argument is
        // not a per-row value and is not walked.
        ast::Expr::Call(call) if is_group_aggregate(call) => true,
        ast::Expr::Call(call) => call
            .args
            .iter()
            .all(|arg| field_walk_is_modeled(&arg.node, keys, offender)),
        ast::Expr::Binary { lhs, rhs, .. } => {
            field_walk_is_modeled(&lhs.node, keys, offender)
                && field_walk_is_modeled(&rhs.node, keys, offender)
        }
        ast::Expr::Prefix { expr, .. } | ast::Expr::Cast { expr, .. } => {
            field_walk_is_modeled(&expr.node, keys, offender)
        }
        ast::Expr::Literal(_) | ast::Expr::Param(_) | ast::Expr::Constant(_) => true,
        _ => false,
    }
}

/// A call the GROUP planner evaluates as an aggregate: the column aggregates
/// of [`is_column_aggregate`] plus every form of `count`.
fn is_group_aggregate(call: &ast::Call) -> bool {
    let path = call.path.node.as_str();
    is_column_aggregate(path) || path == "count"
}

/// 7016 (opt-in, off by default): a `LIMIT`/`START` page cut from rows that
/// have no `ORDER BY` is cut from storage order, so two pages can overlap or
/// skip rows and the same query can answer differently run to run.
///
/// Only a table target pages: a record id is one row and a param/subquery
/// source has no order this statement controls. `ONLY … LIMIT 1` asks for a
/// row, not a page — the `LIMIT 1` is the cardinality proof 4003 wants — and
/// `GROUP ALL` yields a single row, so neither can be misordered.
fn check_page_without_order(stmt: &ast::SelectStmt, ctx: &mut AnalysisContext<'_>) {
    let Some(clause) = stmt.limit.as_ref().or(stmt.start.as_ref()) else {
        return;
    };
    if stmt.order.is_some() || stmt.group.as_ref().is_some_and(|group| group.all) {
        return;
    }
    // `LIMIT 1` with no START is the "any one row" idiom: the author has said
    // which row does not matter, so an unordered cut is not a paging mistake.
    // With ONLY it is also the single-row contract 4003/4026 own.
    if stmt.start.is_none() && literal_limit(stmt).is_some_and(|limit| limit <= 1) {
        return;
    }
    let Some(from) = stmt.from.first() else {
        return;
    };
    if !matches!(from.node, ast::Expr::Table(_)) {
        return;
    }
    let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), clause.span);
    ctx.emit(
        surrealguard_diagnostics::catalog::finding(
            span,
            7016,
            "LIMIT/START without ORDER BY cuts the page from storage order, so which rows it holds is not deterministic"
                .to_string(),
        )
        .with_help(
            "add an `ORDER BY` (e.g. `ORDER BY id`), or allow this with `7016 = \"allow\"` (it is off by default)",
        ),
    );
}

/// The source table's definition when the FROM clause names one plainly.
///
/// A side-effect-free counterpart to [`resolve_from_table`], for the shape
/// checks: those run before the statement resolves its own source, and calling
/// the resolving one twice would emit its findings twice. Anything less direct
/// than a table name or a record id yields `None`, which every caller reads as
/// "prove nothing here".
fn plain_source_table<'a>(
    stmt: &ast::SelectStmt,
    schema: &'a crate::schema::SchemaIndex,
) -> Option<&'a TableDef> {
    let name = match &stmt.from.first()?.node {
        ast::Expr::Table(name) => &name.node,
        ast::Expr::RecordId { table, .. } => &table.node,
        _ => return None,
    };
    schema.tables.get(name)
}

/// The names this query's result rows carry: each projection's `AS` alias,
/// and — for an unaliased plain field projection — its dotted path.
///
/// GROUP BY / ORDER BY keys are resolved against the *result* rows, not the
/// source table: `SELECT price AS n FROM t GROUP BY n` is valid SurrealQL
/// (verified against a live engine) even though `t` has no field `n`. This
/// set is what makes an alias key legal; it is shared by the 1002
/// suppression in both clauses and by 4013's "key isn't projected" check.
fn projected_row_names(stmt: &ast::SelectStmt) -> std::collections::BTreeSet<String> {
    stmt.projections
        .iter()
        .filter_map(|projection| match projection {
            ast::Projection::Expr {
                alias: Some(alias), ..
            } => Some(alias.node.clone()),
            ast::Projection::Expr { expr, alias: None } => match &expr.node {
                ast::Expr::Idiom(idiom) => plain_field_segments(idiom).map(|s| s.join(".")),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Whether a clause key names a projected column — equal to one, or nested
/// under one (projecting `address` covers `GROUP BY address.city`).
fn projected_name_covers(names: &std::collections::BTreeSet<String>, name: &str) -> bool {
    names
        .iter()
        .any(|projected| projected == name || name.starts_with(&format!("{projected}.")))
}

/// A bare zero-argument `count()` in a projection is an aggregate only under
/// a GROUP clause. Without one, SurrealDB evaluates it *per row*, so every
/// row's `count` is the constant `1` — never the row total the author
/// intended. A guard built on the result (`IF $rows = 0 { THROW ... }`) then
/// silently never fires (4023). `GROUP ALL` / `GROUP BY` make it a real
/// aggregate and clear the finding.
///
/// The contract is about the *total*, so it needs the consumer as well as the
/// SELECT: where the result is read only for its cardinality
/// (`cardinality_only`, from [`reads_only_cardinality`]) the ungrouped form is
/// the correct one and the suggested remedy inverts the test.
fn check_count_without_group(
    stmt: &ast::SelectStmt,
    ctx: &mut AnalysisContext<'_>,
    cardinality_only: bool,
) {
    if stmt.group.is_some() || cardinality_only {
        return;
    }
    for projection in &stmt.projections {
        let ast::Projection::Expr { expr, .. } = projection else {
            continue;
        };
        let ast::Expr::Call(call) = &expr.node else {
            continue;
        };
        if is_bare_count(call) {
            let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), expr.span);
            ctx.emit(surrealguard_diagnostics::catalog::finding(
                span,
                4023,
                "count() without GROUP BY yields 1 per row, not a total; add GROUP ALL for a total"
                    .to_string(),
            ));
        }
    }
}

/// The zero-argument row-counting form of `count` (the only spelling 3.2.3
/// parses; `count::count` is "Invalid function/constant path").
fn is_bare_count(call: &ast::Call) -> bool {
    call.args.is_empty() && call.path.node == "count"
}

/// Whether a kind can transitively hold record links (making FETCH
/// meaningful): records themselves, collections/options/unions of them.
/// What `SPLIT` and `FOR` iterate: any array or set. The element kind is `any`
/// because neither position cares what is inside, only that there is an inside.
fn collection_kind() -> Kind {
    Kind::either(vec![
        Kind::Array(Box::new(Kind::Any), None),
        Kind::Set(Box::new(Kind::Any), None),
    ])
}

fn kind_may_hold_record(kind: &Kind) -> bool {
    match kind {
        Kind::Record(_) | Kind::Any | Kind::Object => true,
        Kind::Array(element, _) | Kind::Set(element, _) => kind_may_hold_record(element),
        Kind::Either(variants) => variants.iter().any(kind_may_hold_record),
        _ => false,
    }
}

/// A SELECT is a read; a mutation hiding inside its projections or WHERE
/// is almost never intended (4018).
fn check_read_position_subquery(ctx: &mut AnalysisContext<'_>, expr: &ast::Spanned<ast::Expr>) {
    if let ast::Expr::Subquery(inner) = &expr.node {
        if matches!(
            inner.node,
            ast::Statement::Create(_)
                | ast::Statement::Update(_)
                | ast::Statement::Upsert(_)
                | ast::Statement::Delete(_)
                | ast::Statement::Insert(_)
                | ast::Statement::Relate(_)
        ) {
            let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), expr.span);
            ctx.emit(surrealguard_diagnostics::catalog::finding(
                span,
                4018,
                "this SELECT hides a write; run the mutation as its own statement".to_string(),
            ));
        }
    }
}

fn object_literal(fields: BTreeMap<String, Kind>) -> Kind {
    Kind::Literal(KindLiteral::Object(fields))
}

/// Resolves `FROM person->likes->post` to the traversal's target table.
fn leading_field_table(idiom: &ast::Idiom) -> Option<String> {
    match idiom.parts.first().map(|part| &part.node) {
        Some(ast::IdiomPart::Field(name)) => Some(name.clone()),
        _ => None,
    }
}

fn graph_source_table(idiom: &ast::Idiom, schema: &SchemaIndex) -> Option<String> {
    let (first, rest) = idiom.parts.split_first()?;
    let ast::IdiomPart::Field(source_table) = &first.node else {
        return None;
    };
    resolve_graph_chain(source_table, rest, schema)
}

/// Walks graph steps one hop at a time from `source_table`, returning the final
/// target table. Inline `[WHERE …]` filters are skipped (they narrow rows but
/// preserve the traversal's type). Each `->X` step either steps ONTO the edge
/// table `X` (when the current table sits on the near side of `X`'s relation) or
/// steps FROM the current edge onto its far-side node `X` — so both the common
/// `->edge->node` shape and edge-to-edge chains (`->employee_of->member_of->team`,
/// where `member_of` is a relation `FROM employee_of`) resolve. `None` if any hop
/// can't be proven (prove-or-`Any`).
fn resolve_graph_chain(
    source_table: &str,
    parts: &[ast::Spanned<ast::IdiomPart>],
    schema: &SchemaIndex,
) -> Option<String> {
    let mut current = source_table.to_string();
    let mut stepped = false;
    for part in parts {
        match &part.node {
            // A `[WHERE …]` filter narrows rows without changing the type.
            ast::IdiomPart::Where(_) => continue,
            ast::IdiomPart::Graph { .. } => {
                let (dir, target) = single_graph_target(&part.node)?;
                current = graph_hop_target(&current, dir, target, schema)?;
                stepped = true;
            }
            _ => return None,
        }
    }
    stepped.then_some(current)
}

/// One graph hop from `current` in `dir` to table `next`: either stepping ONTO
/// the edge `next` (when `next`'s relation admits `current` on its near side), or
/// stepping FROM the current edge onto its far-side node `next`. `None` when the
/// schema proves no such connection.
fn graph_hop_target(
    current: &str,
    dir: ast::GraphDir,
    next: &str,
    schema: &SchemaIndex,
) -> Option<String> {
    // Step onto edge table `next` (`->edge`): `next`'s relation admits `current`.
    if relation_accepts_source(current, dir, next, schema) {
        return Some(next.to_string());
    }
    // Step from the current edge onto its far-side node `next` (`->node`).
    let relation = schema
        .tables
        .get(current)
        .and_then(|t| t.relation.as_ref())?;
    let reaches = match dir {
        ast::GraphDir::Out => relation.out_tables.iter().any(|t| t == next),
        ast::GraphDir::In => relation.in_tables.iter().any(|t| t == next),
        ast::GraphDir::Both => {
            relation.out_tables.iter().any(|t| t == next)
                || relation.in_tables.iter().any(|t| t == next)
        }
    };
    reaches.then(|| next.to_string())
}

/// The kind ONE graph step reaches from the value standing in front of it.
///
/// This is what lets the *general* idiom walk
/// ([`crate::analyzer::expression::infer::idiom_prefix_kinds`]) cross a
/// traversal instead of stopping at it. Before it existed, every part behind a
/// `->` was outside the walk's reach, so the only thing that could validate a
/// traversal tail was a hand-written recognizer — and one had to be written per
/// tail form, which is why four of them were missing.
///
/// A step lands on an array: `SELECT ->follows->user FROM user:ada` yields
/// `[user:bob]` even from a single record (3.2.3). Wrappers on the receiver are
/// peeled first — stepping from `array<record<user>>` gives
/// `array<record<follows>>`, not an array of arrays.
///
/// `None` — prove-or-stay-silent — when the step names no single table (`?`,
/// `->(a, b)`, unmodeled syntax, a `<~` reference step), when the receiver is
/// not a record of exactly one table, or when the schema proves no such
/// connection. The step's own findings come from [`super::graph`]; this
/// resolves only the type.
pub(crate) fn graph_step_kind(
    current: &Kind,
    dir: ast::GraphDir,
    step: &ast::GraphStep,
    schema: &SchemaIndex,
) -> Option<Kind> {
    if step.reference || step.wildcard || !step.unmodeled.is_empty() {
        return None;
    }
    let [target] = step.targets.as_slice() else {
        return None;
    };
    let (_, sources) = crate::kinds::record_link_shape(current)?;
    let [source] = sources.as_slice() else {
        return None;
    };
    let landed = graph_hop_target(&source.to_string(), dir, target.node.as_str(), schema)?;
    Some(Kind::Array(
        Box::new(Kind::Record(vec![landed.as_str().into()])),
        None,
    ))
}

/// A graph part's direction and single target table. Multi-target steps
/// (`->(a, b)`) do not resolve to one table and stay unresolved — and neither
/// does a step carrying syntax the AST did not model, because whatever that
/// syntax meant, it was not "the records of this table": a graph selection
/// this lowering could not turn into path parts (`->(SELECT a AS b FROM t)`)
/// hands back projected objects, not links. Resolving it to the target anyway
/// is how that projection came out typed `array<record<t>>`.
fn single_graph_target(part: &ast::IdiomPart) -> Option<(ast::GraphDir, &str)> {
    let ast::IdiomPart::Graph { dir, step } = part else {
        return None;
    };
    if !step.unmodeled.is_empty() {
        return None;
    }
    match step.targets.as_slice() {
        [only] => Some((dir.node, only.node.as_str())),
        _ => None,
    }
}

/// Does `edge`'s relation accept `source_table` on the near side of a step
/// in direction `dir`? (Single-hop edge-field projections need only this.)
fn relation_accepts_source(
    source_table: &str,
    dir: ast::GraphDir,
    edge: &str,
    schema: &SchemaIndex,
) -> bool {
    let Some(relation) = schema.tables.get(edge).and_then(|t| t.relation.as_ref()) else {
        return false;
    };
    match dir {
        ast::GraphDir::Out => relation.in_tables.iter().any(|t| t == source_table),
        ast::GraphDir::In => relation.out_tables.iter().any(|t| t == source_table),
        ast::GraphDir::Both => {
            relation.in_tables.iter().any(|t| t == source_table)
                || relation.out_tables.iter().any(|t| t == source_table)
        }
    }
}

/// `COMPUTED <~T` on table `self_table`: the array of `T` records whose own
/// `REFERENCE` field links back to `self_table`. Resolves to
/// `array<record<T>>` ONLY when the back-reference is provable — a single
/// leading `<~T` reference step (no filter, no trailing parts) where `T` is a
/// defined table carrying a `record<self_table>` `REFERENCE` field. Every
/// other shape yields `None`: the field stays untyped rather than inventing a
/// type we cannot prove from the schema.
///
/// `schema` must be a catalog that can *see* the target: a back-reference is
/// mutual, so `<~task` is proved by a `record<Self> REFERENCE` field on `task`
/// which is as often declared after this field as before it. The sole caller
/// ([`crate::schema::infer_field_value_kind`]) passes the whole-workspace
/// catalog for exactly that reason. What counts as a proof is unchanged.
pub(crate) fn reference_back_traversal_kind(
    self_table: &str,
    idiom: &ast::Idiom,
    schema: &SchemaIndex,
) -> Option<Kind> {
    let (target, indexed) = reference_back_target(idiom)?;
    let target_name = target.node.as_str();
    let target_table = schema.tables.get(target_name)?;
    let points_back = target_table
        .fields
        .values()
        .any(|field| field.reference && kind_targets_table(field.kind.as_ref(), self_table));
    if !points_back {
        return None;
    }
    let element = Kind::Record(vec![target_name.into()]);
    Some(if indexed {
        element
    } else {
        Kind::Array(Box::new(element), None)
    })
}

/// The *syntactic* half of [`reference_back_traversal_kind`]: the table a
/// record-reference back-traversal names, plus whether a single `[index]`
/// subscript follows it.
///
/// A back-reference is `<~T` (the graph step), optionally followed by a single
/// `[index]` subscript that selects ONE element out of the array (`<~T[0]` →
/// `record<T>`, not `array<record<T>>`). A `[WHERE …]` filter or any deeper
/// path is not modeled. This says only which table the step *names* — the
/// table need not exist and need not carry a matching `REFERENCE` field, which
/// is exactly what lets a caller tell the two apart: an unknown table is a
/// typo (1001), a known table with no back-link is genuinely unresolvable.
pub(crate) fn reference_back_target(idiom: &ast::Idiom) -> Option<(&ast::Spanned<String>, bool)> {
    let (graph, indexed) = match idiom.parts.as_slice() {
        [graph] => (graph, false),
        [graph, subscript] if matches!(subscript.node, ast::IdiomPart::Index(_)) => (graph, true),
        _ => return None,
    };
    let ast::IdiomPart::Graph { dir, step } = &graph.node else {
        return None;
    };
    // A back-reference is an incoming reference step (`<~`) at a single named
    // target, with no inline selection.
    if dir.node != ast::GraphDir::In || !step.reference || step.where_clause.is_some() {
        return None;
    }
    let [target] = step.targets.as_slice() else {
        return None;
    };
    Some((target, indexed))
}

/// Whether a field's declared kind is (or wraps, through `option`/union/array)
/// a `record<...>` that names `table`.
fn kind_targets_table(kind: Option<&Kind>, table: &str) -> bool {
    match kind {
        Some(Kind::Record(tables)) => tables.iter().any(|t| t.to_string() == table),
        Some(Kind::Either(variants)) => variants.iter().any(|v| kind_targets_table(Some(v), table)),
        Some(Kind::Array(element, _) | Kind::Set(element, _)) => {
            kind_targets_table(Some(element), table)
        }
        _ => false,
    }
}

fn is_graph_projection(projection: &ast::Projection) -> bool {
    let ast::Projection::Expr { expr, .. } = projection else {
        return false;
    };
    let ast::Expr::Idiom(idiom) = &expr.node else {
        return false;
    };
    starts_with_graph(idiom)
}

// ---------------------------------------------------------------------------
// Projections
// ---------------------------------------------------------------------------

/// Checks a graph projection — **unconditionally**, before anything tries to
/// type it.
///
/// This used to be three calls inline in each projection branch, followed by
/// `if let Some(kind) = graph_projection_kind(…) { … return; }`. That `return`
/// is what made checking conditional on typing: a traversal whose type resolved
/// left the branch before the ordinary expression check ran, so the *better* a
/// projection was understood the *less* it was checked. `->wrote->post.author`
/// resolved (to `array<any>`), returned early, and took `.nope` behind it with
/// it; `->follows->user.john` did not resolve, fell through, and was checked.
/// Exactly inverted.
///
/// Everything a traversal's parts must satisfy is reached from here, through
/// [`crate::analyzer::expression::check::check_value_expression`] — the same
/// entry point every other projected expression goes through.
fn check_graph_projection(
    ctx: &mut AnalysisContext<'_>,
    table: &TableDef,
    expr: &ast::Spanned<ast::Expr>,
) {
    // Re-resolve the table from the schema so the borrow carries the context's
    // lifetime rather than the caller's (as `computed_kind` does).
    let row = ctx.schema().tables.get(&table.name);
    ctx.with_row_table(row, |ctx| {
        crate::analyzer::expression::check::check_value_expression(ctx, expr);
    });
}

/// `SELECT VALUE <expr>` — the row type is the projected value itself.
fn value_projection_kind(
    stmt: &ast::SelectStmt,
    row_table_name: &str,
    table: &'_ TableDef,
    ctx: &mut AnalysisContext<'_>,
) -> Option<Kind> {
    if !stmt.value {
        return None;
    }
    let [ast::Projection::Expr { expr, alias }] = stmt.projections.as_slice() else {
        return None;
    };

    let kind = value_projection_inner_kind(stmt, expr, row_table_name, table, ctx)?;
    // Under `GROUP BY` a non-key, non-aggregate value accumulates each
    // group's rows (4029), so the value is the collected column.
    Some(if group_accumulates(stmt, expr, alias.as_ref()) {
        Kind::Array(Box::new(kind), None)
    } else {
        kind
    })
}

/// The per-row kind of the single `SELECT VALUE` projection.
fn value_projection_inner_kind(
    stmt: &ast::SelectStmt,
    expr: &ast::Spanned<ast::Expr>,
    row_table_name: &str,
    table: &'_ TableDef,
    ctx: &mut AnalysisContext<'_>,
) -> Option<Kind> {
    match &expr.node {
        ast::Expr::Idiom(idiom) if starts_with_graph(idiom) => {
            check_graph_projection(ctx, table, expr);
            graph_projection_kind(row_table_name, idiom, ctx.schema(), false)
        }
        // `SELECT VALUE author.{name}` is the destructured OBJECT itself, not
        // a row keyed by `author`. Without this arm the whole projection fell
        // through to the object path and kept the key: `SELECT VALUE id.{}
        // FROM ONLY user:ada` is `{}` on 3.0.5, not `{id: {}}`.
        ast::Expr::Idiom(idiom)
            if idiom
                .parts
                .iter()
                .any(|part| matches!(part.node, ast::IdiomPart::Destructure(_))) =>
        {
            if let Some((prefix, selected)) = row_destructure_head(idiom) {
                validate_row_destructure(ctx, table, &prefix, selected);
            }
            Some(computed_kind(expr, stmt, table, ctx))
        }
        ast::Expr::Idiom(idiom) => {
            let segments = plain_field_segments(idiom)?;
            // Resolve (crossing record links); validate only when it resolves,
            // so an unresolved head falls through to the object path — which
            // emits E1002 once — rather than double-reporting here.
            let kind = resolve_field_path(ctx.schema(), table, &segments)?;
            validate_field_path(ctx, table, &segments, expr.span, 1002);
            Some(kind)
        }
        _ => Some(computed_kind(expr, stmt, table, ctx)),
    }
}

/// The projected row: every sibling projection layered onto `fields`, which
/// a wildcard pre-seeds with the whole declared row (and is otherwise empty).
fn projected_object_kind(
    stmt: &ast::SelectStmt,
    row_table_name: &str,
    table: &TableDef,
    ctx: &mut AnalysisContext<'_>,
    mut fields: BTreeMap<String, Kind>,
    has_wildcard: bool,
) -> Kind {
    for projection in &stmt.projections {
        match projection {
            ast::Projection::Wildcard(_) => {}
            // The projection itself failed to lower: a poison field keyed by
            // its source text.
            ast::Projection::Partial(partial) => {
                fields.insert(
                    slice(ctx.source_text(), partial.span).to_string(),
                    Kind::Any,
                );
            }
            ast::Projection::Expr { expr, alias } => {
                if group_accumulates(stmt, expr, alias.as_ref()) {
                    // A non-key, non-aggregate projection under `GROUP BY`
                    // (4029) lands as the collected column: project it on
                    // its own, then wrap the value at the key it produced.
                    let mut own = BTreeMap::new();
                    project_expr(
                        expr,
                        alias.as_ref(),
                        stmt,
                        row_table_name,
                        table,
                        ctx,
                        &mut own,
                        false,
                    );
                    let depth = match (alias, &expr.node) {
                        (None, ast::Expr::Idiom(idiom)) => {
                            plain_field_segments(idiom).map_or(1, |segments| segments.len())
                        }
                        _ => 1,
                    };
                    let (path, kind) = accumulated_leaf(own, depth);
                    insert_kind_at_path(&mut fields, &path, Kind::Array(Box::new(kind), None));
                    continue;
                }
                project_expr(
                    expr,
                    alias.as_ref(),
                    stmt,
                    row_table_name,
                    table,
                    ctx,
                    &mut fields,
                    has_wildcard,
                );
                if has_wildcard {
                    if let Some(source) = renamed_away_field(&expr.node, alias.as_ref()) {
                        fields.remove(&source);
                    }
                }
            }
        }
    }

    object_literal(fields)
}

/// Where a lone projection landed in a fresh field map: the path down its
/// single-key chain, at most `depth` segments long, and the kind at the end
/// of it. `address.city` (depth 2) lands as `{address: {city: T}}` and yields
/// `(["address", "city"], T)`; a plain `address` (depth 1) whose kind is an
/// object stops at `(["address"], {…})`, so the wrap goes around the object,
/// not each of its fields.
fn accumulated_leaf(mut fields: BTreeMap<String, Kind>, depth: usize) -> (Vec<String>, Kind) {
    let mut path = Vec::new();
    loop {
        if fields.len() != 1 || path.len() == depth {
            return (path, object_literal(fields));
        }
        let (key, kind) = fields.pop_first().expect("exactly one field");
        path.push(key);
        match kind {
            Kind::Literal(KindLiteral::Object(inner)) if path.len() < depth => fields = inner,
            other => return (path, other),
        }
    }
}

/// The row key a projection *renames away* when a wildcard also supplies the
/// row: `SELECT *, name AS n FROM person` is `{ id, age, n }` — the `name`
/// key is gone, replaced by `n` (3.0.5 live).
///
/// This is a rename, not an addition, and it is deliberately narrow: it fires
/// only for a bare single-segment row field renamed to a different bare
/// single-segment key. SurrealDB's projection planner turns exactly that
/// shape into `Projection::Rename { from, to }`, and `SelectProject` then
/// does `output.remove(from)` when the projection list also holds
/// `Projection::All` ("If we had All, remove the original name to avoid
/// having both `from` and `to` in output"). Everything else stays additive,
/// verified against 3.0.5:
///   - a nested source keeps its parent whole (`*, address.city AS c` →
///     `address` still carries `city` *and* `zip`, plus `c`);
///   - a dotted alias keeps the source (`*, name AS a.b` → both `name` and
///     `a.b`);
///   - a computed source keeps its operands (`*, string::len(name) AS l` →
///     `name` and `l`);
///   - a graph traversal has no row key to remove (`*, ->has_account AS acc`
///     → every field plus `acc`).
///
/// The removal is unconditional on the source *existing*: `SELECT *, nope AS
/// x` simply adds `x`, because removing an absent key is a no-op.
fn renamed_away_field(expr: &ast::Expr, alias: Option<&ast::Spanned<String>>) -> Option<String> {
    let alias = alias?;
    // A dotted alias builds a nested output path, which the planner routes to
    // the general `Project` operator — the one that never removes the source.
    if alias.node.contains('.') {
        return None;
    }
    let ast::Expr::Idiom(idiom) = expr else {
        return None;
    };
    let segments = plain_field_segments(idiom)?;
    let [source] = segments.as_slice() else {
        return None;
    };
    (*source != alias.node).then(|| source.clone())
}

/// The declared row as a field map — what `*` contributes.
fn all_fields_map(table: &TableDef) -> BTreeMap<String, Kind> {
    match object_kind_for_all_fields(table) {
        Kind::Literal(KindLiteral::Object(fields)) => fields,
        _ => BTreeMap::new(),
    }
}

fn project_expr(
    expr: &ast::Spanned<ast::Expr>,
    alias: Option<&ast::Spanned<String>>,
    stmt: &ast::SelectStmt,
    row_table_name: &str,
    table: &TableDef,
    ctx: &mut AnalysisContext<'_>,
    fields: &mut BTreeMap<String, Kind>,
    has_wildcard: bool,
) {
    let alias_name = alias.map(|a| a.node.clone());

    if let ast::Expr::Idiom(idiom) = &expr.node {
        if starts_with_graph(idiom) {
            check_graph_projection(ctx, table, expr);
            // An aliased graph target materializes when FETCHed by alias.
            let materialize = alias_name
                .as_ref()
                .is_some_and(|alias| fetch_contains(&stmt.fetch, alias));
            if let Some(kind) =
                graph_projection_kind(row_table_name, idiom, ctx.schema(), materialize)
            {
                match &alias_name {
                    Some(alias) => {
                        fields.insert(alias.clone(), kind);
                    }
                    None => {
                        if let Some(segments) = graph_output_segments(idiom) {
                            insert_kind_at_path(fields, &segments, kind);
                        }
                    }
                }
                return;
            }
        } else if let Some((prefix, selected)) = row_destructure_parts(idiom) {
            // `profile.{email, city}` (row object) / `team.{label}` (record
            // link) selects sub-fields; each must exist on the target (E1002).
            validate_row_destructure(ctx, table, &prefix, &selected);
            if let Some((wrappers, outputs)) =
                destructure_kinds(ctx.schema(), table, &prefix, &selected)
            {
                // A destructure over a wrapped link projects one object per
                // linked record, so the wrappers apply to the object as a whole.
                let assemble = |outputs: Vec<(Vec<String>, Kind)>| {
                    let object: BTreeMap<String, Kind> = outputs
                        .into_iter()
                        .map(|(segments, kind)| {
                            (segments.last().cloned().unwrap_or_default(), kind)
                        })
                        .collect();
                    crate::kinds::rewrap_kind(&wrappers, object_literal(object))
                };
                // Unaliased under a wildcard the destructure *narrows* the
                // seeded field: `SELECT *, address.{city}` yields `address:
                // { city }` — the unselected `zip` is gone (3.0.5 live).
                // Aliased it is purely additive, and the source stays whole.
                if has_wildcard && alias_name.is_none() {
                    remove_kind_at_path(fields, &prefix);
                }
                match &alias_name {
                    Some(alias) => {
                        fields.insert(alias.clone(), assemble(outputs));
                    }
                    // Unaliased: the object lands at the destructured path
                    // (`members.{name}` -> `members`), carrying its wrappers.
                    None if !wrappers.is_empty() => {
                        let object = assemble(outputs);
                        insert_kind_at_path(fields, &prefix, object);
                    }
                    // An EMPTY selection has no sub-field to land, but the key
                    // is still projected: `SELECT author.{} FROM post` is
                    // `{author: {}}` on 3.0.5, not a row with no `author`.
                    None if outputs.is_empty() => {
                        insert_kind_at_path(fields, &prefix, object_literal(BTreeMap::new()));
                    }
                    None => {
                        for (segments, kind) in outputs {
                            insert_kind_at_path(fields, &segments, kind);
                        }
                    }
                }
                return;
            }
        } else if let Some(segments) = plain_field_segments(idiom) {
            // Validate the path, crossing record links (`team.label` checks
            // `label` on `team`); a valid path is a no-op here.
            validate_field_path(ctx, table, &segments, expr.span, 1002);
            if let Some(kind) = resolve_field_path(ctx.schema(), table, &segments) {
                match &alias_name {
                    Some(alias) => {
                        fields.insert(alias.clone(), kind);
                    }
                    None => insert_kind_at_path(fields, &segments, kind),
                }
                return;
            }
            // Known-plain path that doesn't resolve: poison entry (the finding
            // was already emitted by `validate_field_path`).
            fields.insert(alias_name.unwrap_or_else(|| segments.join(".")), Kind::Any);
            return;
        }

        // A `.{…}` with something behind it (`author.{name}.age`) projects
        // through the generic inference below, which knows nothing of the
        // schema the selection is read against — so its fields are validated
        // here. Guarded on the destructure not being last, because that shape
        // was already validated by the branch above.
        if row_destructure_parts(idiom).is_none() {
            if let Some((prefix, selected)) = row_destructure_head(idiom) {
                validate_row_destructure(ctx, table, &prefix, selected);
            }
        }

        // Idioms with parts the branches above don't project (Start/Index/
        // Method/...): the same full expression inference every other
        // computed projection gets — a method resolves against its receiver's
        // kind (`SELECT name.len()` is an `int`), an index reaches the
        // element kind — with their invariants checked at the same site.
        let kind = computed_kind(expr, stmt, table, ctx);
        match alias_name {
            Some(alias) => {
                fields.insert(alias, kind);
            }
            // Unaliased, the key follows SurrealDB's own simplification rule
            // (`name.len()` → `name`, `tags[0]` → `tags`).
            None => match simplified_key_segments(idiom) {
                Some(segments) => insert_kind_at_path(fields, &segments, kind),
                None => {
                    fields.insert(slice(ctx.source_text(), expr.span).to_string(), kind);
                }
            },
        }
        return;
    }

    // `type::field(path)` / `type::fields([paths])` are *named* projections:
    // SurrealDB expands them into the fields their path strings name, so
    // `type::field('meta.inner')` lands at the nested `meta.inner` and
    // `type::fields(['a', 'b'])` produces both keys (verified on 3.0.5). An
    // alias overrides the expansion, and a path that isn't statically known
    // falls through to the ordinary naming below.
    if alias_name.is_none() {
        if let ast::Expr::Call(call) = &expr.node {
            if let Some(paths) = const_field_path_args(call, ctx) {
                // Check the call as usual (its own 5005 contract) — only the
                // naming differs.
                let _ = computed_kind(expr, stmt, table, ctx);
                for path in paths {
                    let segments: Vec<String> = path.split('.').map(str::to_string).collect();
                    let kind = kind_for_path(table, &segments).unwrap_or(Kind::Any);
                    insert_kind_at_path(fields, &segments, kind);
                }
                return;
            }
        }
    }

    // Computed projection: full expression inference.
    let key = alias_name.unwrap_or_else(|| unaliased_computed_key(expr, ctx.source_text()));
    let kind = computed_kind(expr, stmt, table, ctx);
    fields.insert(key, kind);
}

/// The statically-known field paths a `type::field` / `type::fields` call
/// projects, when its argument is a constant (a literal, or a `LET` binding
/// tracing back to one). `None` when the call is neither, or when the path
/// argument isn't statically a string / list of strings — the projection is
/// then named the ordinary way, since its runtime key is unknowable.
fn const_field_path_args(call: &ast::Call, ctx: &mut AnalysisContext<'_>) -> Option<Vec<String>> {
    use surrealdb_types::Value;

    match call.path.node.as_str() {
        "type::field" => match crate::analyzer::function::const_value_arg(ctx, call, 0)? {
            Value::String(path) => Some(vec![path]),
            _ => None,
        },
        "type::fields" => match crate::analyzer::function::const_value_arg(ctx, call, 0)? {
            Value::Array(paths) => paths
                .iter()
                .map(|value| match value {
                    Value::String(path) => Some(path.clone()),
                    _ => None,
                })
                .collect(),
            _ => None,
        },
        _ => None,
    }
}

/// The result-object key for an unaliased computed projection.
///
/// SurrealDB names a projection after the *top-level* node, not its source
/// text: a call is named by its bare function name — no parens, no arguments
/// (`string::len(name)` → `string::len`, `time::now()` → `time::now`,
/// `fn::abc(age)` → `fn::abc`, `count()` → `count`). Everything else keeps
/// its source text, which is why a call *inside* a larger expression does
/// not shorten it (`math::abs(age) + 1` is a binary, so it stays
/// `"math::abs(age) + 1"`). Verified against SurrealDB 3.0.5.
///
/// A path-less call is a param invocation (`$f(age)`), which the engine names
/// `($f)(age)` — a rendering we don't reproduce; those keep their source text.
pub(crate) fn unaliased_computed_key(expr: &ast::Spanned<ast::Expr>, source_text: &str) -> String {
    if let ast::Expr::Call(call) = &expr.node {
        if !call.path.node.is_empty() {
            return call.path.node.clone();
        }
    }
    slice(source_text, expr.span).to_string()
}

/// The result-object key path of an unaliased idiom projection SurrealDB
/// *simplifies*: only `Field` and `Graph` parts name a key segment, so method
/// calls, indexes, `[WHERE …]` filters, `[*]`/`[$]`, and `?.` contribute
/// nothing (`name.len()` → `name`, `tags[0].len()` → `tags`,
/// `meta.inner.deep.len()` → the nested `meta.inner.deep`,
/// `tags.map(…).a` → the nested `tags.a`,
/// `->knows->person.name.len()` → the nested `->knows.->person.name`).
///
/// `None` for shapes whose simplified rendering isn't a plain path — a
/// leading `Start` value (`$obj.a.len()`, which the engine keys under a
/// literal `$obj` segment), a destructure, or recursion — leaving those on
/// the source-text fallback. Verified against SurrealDB 3.0.5.
pub(crate) fn simplified_key_segments(idiom: &ast::Idiom) -> Option<Vec<String>> {
    if !matches!(
        idiom.parts.first().map(|part| &part.node),
        Some(ast::IdiomPart::Field(_) | ast::IdiomPart::Graph { .. })
    ) {
        return None;
    }
    let mut segments = Vec::new();
    for part in &idiom.parts {
        match &part.node {
            ast::IdiomPart::Field(name) => segments.push(name.clone()),
            ast::IdiomPart::Graph { .. } => {
                let (dir, target) = single_graph_target(&part.node)?;
                let arrow = match dir {
                    ast::GraphDir::Out => "->",
                    ast::GraphDir::In => "<-",
                    ast::GraphDir::Both => "<->",
                };
                segments.push(format!("{arrow}{target}"));
            }
            // Dropped by the engine's simplification.
            ast::IdiomPart::Method { .. }
            | ast::IdiomPart::Index(_)
            | ast::IdiomPart::All
            | ast::IdiomPart::Last
            | ast::IdiomPart::Where(_)
            | ast::IdiomPart::Optional
            | ast::IdiomPart::Flatten => {}
            ast::IdiomPart::Start(_)
            | ast::IdiomPart::Destructure(_)
            | ast::IdiomPart::Recurse { .. }
            | ast::IdiomPart::Partial(_) => return None,
        }
    }
    (!segments.is_empty()).then_some(segments)
}

fn computed_kind(
    expr: &ast::Spanned<ast::Expr>,
    stmt: &ast::SelectStmt,
    table: &TableDef,
    ctx: &mut AnalysisContext<'_>,
) -> Kind {
    if stmt.group.is_some() {
        // An aggregate over a projected column receives the *collected*
        // column, not one row's value — infer it as such so its `array`
        // argument contract is satisfied rather than false-positived.
        if let Some(kind) = aggregate_expression_kind(expr, table, ctx) {
            return kind;
        }
    } else if let Some(kind) = ungrouped_column_aggregate_kind(expr, table, ctx) {
        return kind;
    }
    // Re-resolve the table from the schema so the borrow carries the
    // context's lifetime rather than the caller's.
    let table = ctx.schema().tables.get(&table.name);
    ctx.with_row_table(table, |ctx| {
        crate::analyzer::expression::check::check_value_expression(ctx, expr);
        infer_expression_fact(expr, ctx).kind.unwrap_or(Kind::Any)
    })
}

/// Aggregate functions collapse a *column collected across rows* into a
/// single value. In a projection the author writes one row's value
/// (`math::sum(size_bytes)`), but the aggregate is handed the whole column —
/// so the argument's per-row kind is promoted to `array<per-row>` before the
/// signature runs. Without this, the per-row `int` reading of `size_bytes`
/// violates the `array` argument contract and false-positives (5002).
///
/// The argument may be any expression: SurrealDB's aggregate operator holds
/// an `argument_expr` it evaluates per row (`math::sum(price * qty)` is as
/// valid as `math::sum(price)`), and the aggregate itself may sit at any
/// depth in the projection (`math::sum(a) + math::sum(b)`), which
/// [`aggregate_expression_kind`] models.
///
/// Returns `None` only when the shape is one the caller should infer the
/// ordinary way: not a single-argument call, or a plain column that is
/// *already* a collection (`math::max(tags)` keeps its element-wise
/// reading).
fn column_aggregate_kind(
    call: &ast::Call,
    table: &TableDef,
    ctx: &mut AnalysisContext<'_>,
) -> Option<Kind> {
    let [arg] = call.args.as_slice() else {
        return None;
    };
    // A plain column resolves straight from the schema — the established
    // path, kept exactly as it was so its (absence of) findings is unchanged.
    let plain_column = match &arg.node {
        ast::Expr::Idiom(idiom) => {
            plain_field_segments(idiom).and_then(|segments| kind_for_path(table, &segments))
        }
        _ => None,
    };
    let (column, already_checked) = match plain_column {
        Some(column) => (column, false),
        None => {
            // Any other argument is one row's value: infer it under the row
            // context, with its own invariants checked (the ordinary path
            // would have checked it as part of the call).
            let row_table = ctx.schema().tables.get(&table.name);
            let kind = ctx.with_row_table(row_table, |ctx| {
                crate::analyzer::expression::check::check_value_expression(ctx, arg);
                infer_expression_fact(arg, ctx).kind.unwrap_or(Kind::Any)
            });
            (kind, true)
        }
    };
    let base = crate::kinds::literal_base_kind(&column).unwrap_or_else(|| column.clone());
    let collected = if matches!(base, Kind::Array(_, _) | Kind::Set(_, _)) {
        // Already a collection: the ordinary element-wise contract fits.
        if !already_checked {
            return None;
        }
        column
    } else {
        Kind::Array(Box::new(column), None)
    };
    Some(crate::analyzer::function::analyze_builtin_function(
        ctx,
        call,
        &[collected],
    ))
}

/// 4028: without a GROUP clause there is no column to collect, so an
/// aggregate written over a scalar row field (`SELECT math::sum(age) FROM
/// person`) runs per row on one `int` — the engine rejects the argument
/// (`Expected an array`), and no total was ever going to come back. Only the
/// provable shape is reported: every aggregate in the projection takes a
/// plain row column whose declared kind is definitely not a collection. A
/// collection column (`math::sum(tags)` over `array<int>`), a parameter, or
/// a computed argument is inferred the ordinary way and stays silent here —
/// the ordinary signature contract (5002) covers what it can.
///
/// Returns the promoted kind for the shape it reports, so the same call is
/// not also reported against its signature: one mistake, one code. `count()`
/// is not in the aggregate set (4023 owns it).
fn ungrouped_column_aggregate_kind(
    expr: &ast::Spanned<ast::Expr>,
    table: &TableDef,
    ctx: &mut AnalysisContext<'_>,
) -> Option<Kind> {
    if !contains_column_aggregate(&expr.node) || !models_aggregate_shape(&expr.node) {
        return None;
    }
    let mut calls = Vec::new();
    collect_column_aggregates(expr, &mut calls);
    let mut scalar_columns = Vec::with_capacity(calls.len());
    for call in &calls {
        let [arg] = call.node.args.as_slice() else {
            return None;
        };
        let ast::Expr::Idiom(idiom) = &arg.node else {
            return None;
        };
        let segments = plain_field_segments(idiom)?;
        let column = kind_for_path(table, &segments)?;
        if !is_definitely_scalar(&column) {
            return None;
        }
        scalar_columns.push((segments.join("."), column));
    }
    for (call, (column_name, column)) in calls.iter().zip(&scalar_columns) {
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), call.span);
        ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                span,
                4028,
                format!(
                    "`{}({column_name})` runs per row without a GROUP clause — `{column_name}` is one `{}`, not a column, and SurrealDB rejects the call",
                    call.node.path.node,
                    crate::render_kind(column),
                ),
            )
            .with_help(
                "add `GROUP ALL` for a total over every row, or `GROUP BY <key>` for one per group",
            ),
        );
    }
    aggregate_operand_kind(expr, table, ctx)
}

/// Every aggregate call in an expression whose shape [`models_aggregate_shape`]
/// accepted (so none is nested inside another).
fn collect_column_aggregates<'e>(
    expr: &'e ast::Spanned<ast::Expr>,
    out: &mut Vec<ast::Spanned<&'e ast::Call>>,
) {
    match &expr.node {
        ast::Expr::Call(call) if is_column_aggregate(call.path.node.as_str()) => {
            out.push(ast::Spanned {
                node: call,
                span: expr.span,
            });
        }
        ast::Expr::Call(call) => {
            for arg in &call.args {
                collect_column_aggregates(arg, out);
            }
        }
        ast::Expr::Binary { lhs, rhs, .. } => {
            collect_column_aggregates(lhs, out);
            collect_column_aggregates(rhs, out);
        }
        ast::Expr::Prefix { expr, .. } | ast::Expr::Cast { expr, .. } => {
            collect_column_aggregates(expr, out);
        }
        _ => {}
    }
}

/// Whether a declared kind is provably a single value — never a collection,
/// and never something (`any`, an open object, a function) that might hold
/// one. `option<int>` is scalar: NONE per row is still not a column.
fn is_definitely_scalar(kind: &Kind) -> bool {
    match kind {
        Kind::Any
        | Kind::Object
        | Kind::Array(..)
        | Kind::Set(..)
        | Kind::Function(..)
        | Kind::Range
        | Kind::Literal(KindLiteral::Array(_) | KindLiteral::Object(_)) => false,
        Kind::Either(variants) => !variants.is_empty() && variants.iter().all(is_definitely_scalar),
        _ => true,
    }
}

/// A projection may compute *around* an aggregate — `math::sum(age) * 2`,
/// `math::sum(price) + math::sum(qty)`, `<float> math::sum(x)`. SurrealDB's
/// planner extracts the aggregate call from any depth and evaluates the
/// surrounding expression over its result, so the whole projection is valid;
/// inferring it per-row instead makes every one of those aggregates
/// false-positive on its `array` argument contract (5002).
///
/// Returns `None` when the projection carries no aggregate, or carries one
/// in a position this doesn't model (an aggregate nested in another
/// aggregate — which SurrealDB itself rejects — or inside a container
/// literal): the caller then falls back to ordinary per-row inference,
/// exactly as before.
fn aggregate_expression_kind(
    expr: &ast::Spanned<ast::Expr>,
    table: &TableDef,
    ctx: &mut AnalysisContext<'_>,
) -> Option<Kind> {
    if !contains_column_aggregate(&expr.node) || !models_aggregate_shape(&expr.node) {
        return None;
    }
    aggregate_operand_kind(expr, table, ctx)
}

/// One operand of an aggregate-bearing projection: the aggregate-carrying
/// parts collapse the column, the rest are ordinary per-row values.
/// Shape support is decided up front by [`models_aggregate_shape`], so this
/// only returns `None` where that predicate already allows a fall-back.
fn aggregate_operand_kind(
    expr: &ast::Spanned<ast::Expr>,
    table: &TableDef,
    ctx: &mut AnalysisContext<'_>,
) -> Option<Kind> {
    if !contains_column_aggregate(&expr.node) {
        let row_table = ctx.schema().tables.get(&table.name);
        return Some(ctx.with_row_table(row_table, |ctx| {
            crate::analyzer::expression::check::check_value_expression(ctx, expr);
            infer_expression_fact(expr, ctx).kind.unwrap_or(Kind::Any)
        }));
    }
    match &expr.node {
        ast::Expr::Call(call) => column_aggregate_kind(call, table, ctx),
        ast::Expr::Binary { lhs, op, rhs } => {
            let lhs_kind = aggregate_operand_kind(lhs, table, ctx)?;
            let rhs_kind = aggregate_operand_kind(rhs, table, ctx)?;
            Some(
                crate::analyzer::expression::infer::binary_result_kind(
                    &op.node, &lhs_kind, &rhs_kind,
                )
                .unwrap_or(Kind::Any),
            )
        }
        ast::Expr::Prefix { op, expr: operand } => {
            let operand_kind = aggregate_operand_kind(operand, table, ctx)?;
            Some(match op.node {
                ast::PrefixOp::Not => Kind::Bool,
                _ if crate::analyzer::expression::infer::is_numeric(&operand_kind) => operand_kind,
                _ => Kind::Any,
            })
        }
        ast::Expr::Cast { ty, expr: inner } => {
            aggregate_operand_kind(inner, table, ctx)?;
            Some(
                crate::analyzer::expression::infer::cast_target_kind(&ty.node).unwrap_or(Kind::Any),
            )
        }
        _ => None,
    }
}

/// Whether an aggregate-bearing expression is one whose surrounding
/// computation is modeled. Decided structurally *before* anything is
/// inferred so a fall-back to ordinary inference never double-reports the
/// findings of a part already walked.
fn models_aggregate_shape(expr: &ast::Expr) -> bool {
    fn part(expr: &ast::Spanned<ast::Expr>) -> bool {
        !contains_column_aggregate(&expr.node) || models_aggregate_shape(&expr.node)
    }
    match expr {
        // A nested aggregate (`math::sum(math::max(x))`) is rejected by
        // SurrealDB itself; leave it to ordinary inference.
        ast::Expr::Call(call) => {
            is_column_aggregate(call.path.node.as_str())
                && call.args.len() == 1
                && !contains_column_aggregate(&call.args[0].node)
        }
        ast::Expr::Binary { lhs, rhs, .. } => part(lhs) && part(rhs),
        ast::Expr::Prefix { expr, .. } => part(expr),
        ast::Expr::Cast { expr, .. } => part(expr),
        _ => false,
    }
}

/// Whether an aggregate call appears anywhere in an expression.
fn contains_column_aggregate(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Call(call) => {
            is_column_aggregate(call.path.node.as_str())
                || call
                    .args
                    .iter()
                    .any(|arg| contains_column_aggregate(&arg.node))
        }
        ast::Expr::Binary { lhs, rhs, .. } => {
            contains_column_aggregate(&lhs.node) || contains_column_aggregate(&rhs.node)
        }
        ast::Expr::Prefix { expr, .. } | ast::Expr::Cast { expr, .. } => {
            contains_column_aggregate(&expr.node)
        }
        ast::Expr::Array(elements) => elements
            .iter()
            .any(|element| contains_column_aggregate(&element.node)),
        ast::Expr::Object(fields) => fields
            .iter()
            .any(|(_, value)| contains_column_aggregate(&value.node)),
        _ => false,
    }
}

/// Functions SurrealDB evaluates as *aggregates*: the projection's per-row
/// value is collected across the group and the function is handed the whole
/// column, so a scalar projected column must be collected first.
///
/// The membership is the engine's, not a shape heuristic — verified on 3.0.5
/// with a two-row group (`tier` × `name: string`, `qty: int`):
///
/// | projection | result | verdict |
/// |---|---|---|
/// | `math::sum(qty)` | `7` | reduced ⇒ aggregate |
/// | `array::distinct(qty)` | `[2, 5]` | the collected column ⇒ aggregate |
/// | `array::group(qty)` | `[2, 5]` | the collected column ⇒ aggregate |
/// | `time::min(created)` | one datetime | reduced ⇒ aggregate |
/// | `array::flatten(qty)` | `[null, null]` | per-row ⇒ **not** an aggregate |
/// | `array::first(qty)` | `[null, null]` | per-row ⇒ **not** an aggregate |
/// | `array::len(tags)` | `[2, 2]` | per-row ⇒ **not** an aggregate |
/// | `array::sort(qty)` | `[null, null]` | per-row ⇒ **not** an aggregate |
///
/// (`count` is not listed: a bare `count()` takes no column, and `count(x)`
/// needs no promotion — it accepts any argument.)
fn is_column_aggregate(path: &str) -> bool {
    matches!(
        path,
        "math::sum"
            | "math::mean"
            | "math::min"
            | "math::max"
            | "math::median"
            | "math::mode"
            | "math::product"
            | "math::stddev"
            | "math::variance"
            | "math::spread"
            | "math::midhinge"
            | "math::trimean"
            | "math::interquartile"
            // Collectors: the aggregator hands these the column itself.
            // `array::distinct` dedupes it, `array::group` returns it as-is.
            | "array::distinct"
            | "array::group"
            // Ordering aggregates over a datetime column.
            | "time::min"
            | "time::max"
    )
}

// ---------------------------------------------------------------------------
// Graph projections
// ---------------------------------------------------------------------------

pub(crate) fn is_graph_projection_idiom(idiom: &ast::Idiom) -> bool {
    starts_with_graph(idiom)
}

fn starts_with_graph(idiom: &ast::Idiom) -> bool {
    matches!(
        idiom.parts.first().map(|p| &p.node),
        Some(ast::IdiomPart::Graph { .. })
    )
}

/// Splits a graph idiom into its leading graph section (graph steps plus any
/// interleaved `[WHERE …]` filters) and the projected tail (fields/destructure).
fn graph_split(
    idiom: &ast::Idiom,
) -> (
    &[ast::Spanned<ast::IdiomPart>],
    &[ast::Spanned<ast::IdiomPart>],
) {
    let boundary = idiom
        .parts
        .iter()
        .position(|part| {
            !matches!(
                part.node,
                ast::IdiomPart::Graph { .. } | ast::IdiomPart::Where(_)
            )
        })
        .unwrap_or(idiom.parts.len());
    idiom.parts.split_at(boundary)
}

/// The graph steps within a graph section, dropping interleaved `[WHERE …]`
/// filters (which narrow rows but preserve the traversal's type).
fn graph_steps(section: &[ast::Spanned<ast::IdiomPart>]) -> Vec<&ast::Spanned<ast::IdiomPart>> {
    section
        .iter()
        .filter(|part| matches!(part.node, ast::IdiomPart::Graph { .. }))
        .collect()
}

/// Where a traversal tail stops being a *path* and starts being a *shape*.
///
/// Everything up to the first `.{…}` reads fields off a table; the destructure
/// then replaces the value with an object of its own, and every part after it
/// steps into that object rather than into the table. Splitting once, here,
/// is what keeps the two halves from being resolved by two different rules.
fn tail_split(
    tail: &[ast::Spanned<ast::IdiomPart>],
) -> (
    &[ast::Spanned<ast::IdiomPart>],
    &[ast::Spanned<ast::IdiomPart>],
) {
    match tail
        .iter()
        .position(|part| matches!(part.node, ast::IdiomPart::Destructure(_)))
    {
        Some(at) => tail.split_at(at),
        None => (tail, &[]),
    }
}

/// The kind a traversal's field tail reaches, *before* any `.*` is applied and
/// before any `.{…}` reshapes it, paired with the wildcard part when the tail
/// writes one.
///
/// `->likes.since` reads off the *edge*, `->likes->post.title` off the landed
/// target, and a tail that is nothing but the wildcard — or nothing but a
/// destructure — stands on the row itself (`record<post>`), which is exactly
/// what the splat expands and what the destructure selects from. Sharing this
/// between the type and its check is what keeps the reported table and the
/// inferred kind from drifting apart.
fn graph_tail_receiver<'i>(
    row_table_name: &str,
    idiom: &'i ast::Idiom,
    schema: &SchemaIndex,
) -> Option<(Kind, Option<&'i ast::Spanned<ast::IdiomPart>>)> {
    let (graphs, tail) = graph_split(idiom);
    let steps = graph_steps(graphs);
    let (path, _) = tail_split(tail);
    let segments = tail_path_segments(path)?;
    let wildcard = path
        .iter()
        .find(|part| matches!(part.node, ast::IdiomPart::All));

    // A single hop *with* a tail reads off the edge; a wildcard changes what
    // is projected, not which table it comes from. When the hop is not onto an
    // edge at all — `->user.{name}` standing on a `follows` row already steps
    // off the edge onto its far side — the ordinary chain resolution answers.
    let (table, name) = single_hop_edge(row_table_name, &steps, tail, schema)
        .or_else(|| resolve_graph_chain(row_table_name, graphs, schema))
        .and_then(|name| Some((schema.tables.get(&name)?, name)))?;

    let reached = if segments.is_empty() {
        Kind::Record(vec![name.as_str().into()])
    } else {
        // Schema-aware, so a tail that crosses the landed row's own link
        // resolves rather than stopping at it: `->employee_of->organization
        // .owner.username` is a `string`, and `->employee_of.out.name` reads
        // through the edge's implicit `out`. `kind_for_path` answers `Any` past
        // any link, which is a silent `any` at exactly the sites this walk is
        // meant to type.
        resolve_field_path(schema, table, &segments)?
    };
    Some((reached, wildcard))
}

/// The edge table a single-hop traversal with a tail reads off (`->likes.since`
/// is `likes`'s `since`), when the hop really is onto an edge that admits the
/// row.
fn single_hop_edge(
    row_table_name: &str,
    steps: &[&ast::Spanned<ast::IdiomPart>],
    tail: &[ast::Spanned<ast::IdiomPart>],
    schema: &SchemaIndex,
) -> Option<String> {
    if steps.len() != 1 || tail.is_empty() {
        return None;
    }
    let (dir, edge) = single_graph_target(&steps[0].node)?;
    relation_accepts_source(row_table_name, dir, edge, schema).then(|| edge.to_string())
}

/// Steps one *reshaping* tail part over the kind in hand, schema-only.
///
/// This is the half of the idiom walk a traversal tail can do without an
/// analysis context: a `.{…}` builds an object from the value it stands on, a
/// `.*` expands it, a field reads into it. Anything else (an index, a filter,
/// a method) is not resolved here and leaves the caller with `None`.
fn step_reshaping_part(
    current: &Kind,
    part: &ast::IdiomPart,
    schema: &SchemaIndex,
) -> Option<Kind> {
    match part {
        ast::IdiomPart::Destructure(selected) => {
            let mut object = BTreeMap::new();
            for sub in selected {
                let segments = plain_field_segments(&sub.node)?;
                // A field absent on the target still projects (as `Any`);
                // `validate_graph_destructure` reports it separately.
                object.insert(
                    segments.join("."),
                    kind_at_sub_path(current, &segments, schema).unwrap_or(Kind::Any),
                );
            }
            Some(object_literal(object))
        }
        ast::IdiomPart::Field(name) => {
            crate::analyzer::expression::infer::field_of_kind(current, name, schema)
        }
        ast::IdiomPart::All => Some(
            crate::analyzer::expression::infer::splat_kind(current, schema).unwrap_or(Kind::Any),
        ),
        _ => None,
    }
}

/// A destructure's sub-path resolved off whatever the destructure stands on —
/// a record link (entered through the schema, crossing further links) or the
/// object a *nested* destructure leaves behind: [`crate::kinds::project_fields`].
fn kind_at_sub_path(current: &Kind, segments: &[String], schema: &SchemaIndex) -> Option<Kind> {
    crate::kinds::project_fields(current, segments, Some(schema))
}

/// The type of one graph projection (`->likes->post`, `->likes.since`,
/// `->likes->post.*`, `->likes->post.{a}`, `->likes->post.{a}.a`), wrapped in
/// the traversal's array.
pub(crate) fn graph_projection_kind(
    row_table_name: &str,
    idiom: &ast::Idiom,
    schema: &SchemaIndex,
    materialize_target: bool,
) -> Option<Kind> {
    let (graphs, tail) = graph_split(idiom);

    let (reached, wildcard) = graph_tail_receiver(row_table_name, idiom, schema)?;
    let mut projected = if wildcard.is_some() {
        // `.*` expands the row the tail stands on, exactly as `SELECT *` does.
        // A row with no declared shape leaves the projection `any`, which
        // `validate_graph_wildcard` reports.
        crate::analyzer::expression::infer::splat_kind(&reached, schema).unwrap_or(Kind::Any)
    } else if tail.is_empty() && materialize_target {
        // A FETCHed alias hands back the landed row rather than a link to it.
        let target_name = resolve_graph_chain(row_table_name, graphs, schema)?;
        object_kind_for_all_fields(schema.tables.get(&target_name)?)
    } else {
        reached
    };

    // `.{…}` and everything after it: the destructure builds an object off the
    // row the path reached, and a field after it reads out of *that* object.
    // Falling out of this loop with `None` is what used to happen to the whole
    // projection the moment a destructure appeared with anything behind it.
    let (_, reshaping) = tail_split(tail);
    for part in reshaping {
        projected = step_reshaping_part(&projected, &part.node, schema)?;
    }

    Some(Kind::Array(Box::new(projected), None))
}

/// Output key segments for an unaliased graph projection: `->likes`,
/// `->post`, then any projected tail fields.
fn graph_output_segments(idiom: &ast::Idiom) -> Option<Vec<String>> {
    let (graphs, tail) = graph_split(idiom);
    let mut segments = graph_segments_of(graphs)?;
    segments.extend(tail_key_segments(tail).unwrap_or_default());
    Some(segments)
}

fn graph_segments_of(graphs: &[ast::Spanned<ast::IdiomPart>]) -> Option<Vec<String>> {
    graph_steps(graphs)
        .iter()
        .map(|part| {
            let (dir, target) = single_graph_target(&part.node)?;
            let arrow = match dir {
                ast::GraphDir::Out => "->",
                ast::GraphDir::In => "<-",
                ast::GraphDir::Both => "<->",
            };
            Some(format!("{arrow}{target}"))
        })
        .collect()
}

/// A traversal tail's field path, with any `.*` dropped.
///
/// A wildcard in a tail is not a path segment — it names no field and no
/// output key (the engine simplifies `->follows->user.*.name` to the key
/// `->follows.->user.name`, verified on 3.0.5). It is an instruction:
/// *expand the link in front of me into the row it names*. So it is stripped
/// from the path here and re-applied to the resolved kind as a splat.
///
/// `None` when the tail holds a part that is neither field nor wildcard — an
/// index, a filter, a method — which the caller resolves its own way.
fn tail_path_segments(tail: &[ast::Spanned<ast::IdiomPart>]) -> Option<Vec<String>> {
    let mut segments = Vec::new();
    for part in tail {
        match &part.node {
            ast::IdiomPart::Field(name) => segments.push(name.clone()),
            ast::IdiomPart::All => {}
            _ => return None,
        }
    }
    Some(segments)
}

/// A traversal tail's *output key*, with `.*` and `.{…}` dropped.
///
/// Neither names a key. The engine keys `->follows->user.{name}` under
/// `->follows.->user` (holding the whole selected object) and
/// `->follows->user.{name}.name` under `->follows.->user.name` — the
/// destructure contributes nothing, the field behind it contributes its name.
/// Verified on 3.0.5.
fn tail_key_segments(tail: &[ast::Spanned<ast::IdiomPart>]) -> Option<Vec<String>> {
    let mut segments = Vec::new();
    for part in tail {
        match &part.node {
            ast::IdiomPart::Field(name) => segments.push(name.clone()),
            ast::IdiomPart::All | ast::IdiomPart::Destructure(_) => {}
            _ => return None,
        }
    }
    Some(segments)
}

/// The FIRST `.{…}` in a row idiom together with the plain field path in front
/// of it, whether or not anything follows it.
///
/// [`row_destructure_parts`] answers only the projecting shape (the
/// destructure last); this answers the *validating* one. `author.{name}.age`
/// still selects `name` off `author`, and a selection that names a field the
/// target does not declare is wrong there too — it was silent purely because
/// the destructure was not the final part.
fn row_destructure_head(idiom: &ast::Idiom) -> Option<(Vec<String>, &[ast::Spanned<ast::Idiom>])> {
    let at = idiom
        .parts
        .iter()
        .position(|part| matches!(part.node, ast::IdiomPart::Destructure(_)))?;
    let ast::IdiomPart::Destructure(selected) = &idiom.parts[at].node else {
        return None;
    };
    let prefix = idiom.parts[..at]
        .iter()
        .map(|part| match &part.node {
            ast::IdiomPart::Field(name) => Some(name.clone()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some((prefix, selected.as_slice()))
}

/// `profile.{email, city}`: leading plain fields plus a trailing destructure.
fn row_destructure_parts(
    idiom: &ast::Idiom,
) -> Option<(Vec<String>, Vec<ast::Spanned<ast::Idiom>>)> {
    let (last, prefix) = idiom.parts.split_last()?;
    let ast::IdiomPart::Destructure(selected) = &last.node else {
        return None;
    };
    let prefix_segments = prefix
        .iter()
        .map(|part| match &part.node {
            ast::IdiomPart::Field(name) => Some(name.clone()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some((prefix_segments, selected.clone()))
}

/// The wrappers peeled off a destructured link (re-applied to the whole
/// projected object) and the resolved `(path, kind)` of each selected field.
type HoistedLink = (Vec<crate::kinds::KindWrapper>, Vec<(Vec<String>, Kind)>);

/// The selected sub-field kinds of a `.{…}` destructure, plus any wrappers that
/// belong to the **whole projected object** rather than to its fields.
///
/// Destructuring a *wrapped* link (`members.{name}` where `members` is
/// `array<record<user>>`) yields one object per linked record — SurrealDB
/// returns `array<{ name: string }>`, not `{ name: array<string> }`. So the
/// `option`/`array`/`set` layers are peeled off the link here, the fields are
/// resolved against the link target unwrapped, and the layers are handed back
/// for the caller to re-apply to the assembled object.
fn destructure_kinds(
    schema: &SchemaIndex,
    table: &TableDef,
    prefix: &[String],
    selected: &[ast::Spanned<ast::Idiom>],
) -> Option<HoistedLink> {
    // Only a wrapped link hoists; a bare `record<T>` link and a plain nested
    // object both keep today's field-by-field resolution.
    let wrapped_link =
        record_link_targets_at(table, prefix).filter(|(wrappers, _)| !wrappers.is_empty());

    let mut outputs = Vec::new();
    for sub in selected {
        let sub_segments = plain_field_segments(&sub.node)?;
        let kind = match &wrapped_link {
            // Resolve against the link target itself, so the field carries no
            // trace of the collection/option it was reached through.
            Some((_, targets)) => resolve_across_link(schema, targets, &sub_segments),
            // A field absent on the target still projects (as `Any`);
            // `validate_row_destructure` reports it separately.
            None => {
                let mut segments = prefix.to_vec();
                segments.extend(sub_segments.iter().cloned());
                resolve_field_path(schema, table, &segments).unwrap_or(Kind::Any)
            }
        };
        let mut segments = prefix.to_vec();
        segments.extend(sub_segments);
        outputs.push((segments, kind));
    }
    let wrappers = wrapped_link
        .map(|(wrappers, _)| wrappers)
        .unwrap_or_default();
    Some((wrappers, outputs))
}

/// Names the tail part that stopped the traversal from being typed.
///
/// [`graph_projection_kind`] models exactly three kinds of tail part — a field,
/// a `.*`, and a `.{…}` — and answers `None` for everything else, which reaches
/// the author as a bare `any` with no finding: the projection *looks* analyzed.
/// Every other reason a traversal cannot be typed already says so (6003 at the
/// step, 3001/3002 at the edge); this is the tail's half of the same contract.
///
/// The match is exhaustive on purpose. The three modelled forms are listed as
/// themselves rather than caught by a wildcard, so a new `IdiomPart` cannot be
/// added without someone deciding which side of this line it falls on — and the
/// unmodelled side is *reported*, never silent.
pub(crate) fn validate_graph_tail(ctx: &mut AnalysisContext<'_>, idiom: &ast::Idiom) {
    // `graph_split` only means anything for an idiom that starts with a step.
    if !starts_with_graph(idiom) {
        return;
    }
    let (_, tail) = graph_split(idiom);
    for part in tail {
        let unmodelled = match &part.node {
            // Modelled: these resolve, and their contents are checked.
            ast::IdiomPart::Field(_) | ast::IdiomPart::All => continue,
            // A destructure is modelled when every entry is a plain field
            // path. `.{author.name}` is not one: SurrealDB rejects it outright
            // ("expected a `*` or a destructuring", 3.2.3) and our own grammar
            // lowers the `.name` to an unmodelled subscript, so the whole
            // projection came back `any` with nothing said.
            ast::IdiomPart::Destructure(selected) => {
                if selected
                    .iter()
                    .all(|sub| plain_field_segments(&sub.node).is_some())
                {
                    continue;
                }
                "a destructure entry that is not a plain field name"
            }
            ast::IdiomPart::Index(_) => "an index",
            ast::IdiomPart::Last => "`[$]`",
            ast::IdiomPart::Where(_) => "a filter behind a field",
            ast::IdiomPart::Method { .. } => "a method call",
            ast::IdiomPart::Recurse { .. } => "a recursion",
            ast::IdiomPart::Optional => "`?`",
            ast::IdiomPart::Flatten => "`...`",
            ast::IdiomPart::Graph { .. } => "a further traversal step",
            ast::IdiomPart::Start(_) => "a leading value",
            ast::IdiomPart::Partial(_) => "syntax that did not lower",
        };
        ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), part.span),
                6003,
                format!(
                    "surrealguard can't type what follows this traversal — {unmodelled} is not modelled here"
                ),
            )
            .with_help(
                "a field path, `.*` and `.{…}` after a traversal are typed; the rest is left open",
            ),
        );
        // One finding per traversal: the first unmodelled part is why the
        // whole projection is `any`, and the parts behind it are moot.
        return;
    }
}

/// Reports a `.*` on a traversal whose row has no declared shape
/// (`->wrote->page.*` where `page` declares no fields): the projection's type
/// is `any`, and a bare `any` with no finding reads as "analyzed
/// successfully". Same contract, same code (7008) as a bare `SELECT *` on
/// that table. Silent when the traversal resolves to a shaped row, and when
/// it does not resolve at all — that already has its own findings.
pub(crate) fn validate_graph_wildcard(
    ctx: &mut AnalysisContext<'_>,
    row_table_name: &str,
    idiom: &ast::Idiom,
) {
    let Some((reached, Some(wildcard))) = graph_tail_receiver(row_table_name, idiom, ctx.schema())
    else {
        return;
    };
    if let Some(table) =
        crate::analyzer::expression::infer::unexpandable_splat_table(&reached, ctx.schema())
    {
        crate::analyzer::expression::check::emit_fieldless_splat(ctx, wildcard.span, &table);
    }
}

/// Validates each selected sub-field of a `.{…}` destructure on a graph target
/// (`->friend->user.{name, aeg}`, `->friend->user.{name, aeg}.name`): resolves
/// the row the destructure stands on and checks each field there (E1002),
/// crossing record links. Silent when that row can't be resolved or isn't in
/// the schema (no false positives).
pub(crate) fn validate_graph_destructure(
    ctx: &mut AnalysisContext<'_>,
    row_table_name: &str,
    idiom: &ast::Idiom,
) {
    let (_, tail) = graph_split(idiom);
    let (_, reshaping) = tail_split(tail);
    // `->follows->user.{name}.age` reads a key the selection does not hold.
    crate::analyzer::expression::check::check_field_after_destructure(ctx, reshaping);
    let Some(ast::IdiomPart::Destructure(selected)) = reshaping.first().map(|part| &part.node)
    else {
        return;
    };
    let schema = ctx.schema();
    // `graph_tail_receiver` stops exactly where the destructure begins, so the
    // table it names is the one the selection is read from.
    let Some((Kind::Record(targets), _)) = graph_tail_receiver(row_table_name, idiom, schema)
    else {
        return;
    };
    let [target_name] = targets.as_slice() else {
        return;
    };
    let Some(target) = schema.tables.get(&target_name.to_string()) else {
        return;
    };
    for sub in selected {
        if let Some(segments) = plain_field_segments(&sub.node) {
            validate_field_path(ctx, target, &segments, sub.span, 1002);
        }
    }
}

/// Validates each selected sub-field of a row-field destructure
/// (`profile.{email, nope}`, `team.{label}`) against the target table,
/// crossing record links. Emits E1002 for a field absent on a known target.
fn validate_row_destructure(
    ctx: &mut AnalysisContext<'_>,
    table: &TableDef,
    prefix: &[String],
    selected: &[ast::Spanned<ast::Idiom>],
) {
    for sub in selected {
        if let Some(sub_segments) = plain_field_segments(&sub.node) {
            let mut segments = prefix.to_vec();
            segments.extend(sub_segments);
            validate_field_path(ctx, table, &segments, sub.span, 1002);
        }
    }
}

// ---------------------------------------------------------------------------
// Modifier transforms
// ---------------------------------------------------------------------------

fn idiom_segments(idioms: &[ast::Spanned<ast::Idiom>]) -> Vec<Vec<String>> {
    idioms
        .iter()
        .filter_map(|idiom| plain_field_segments(&idiom.node))
        .collect()
}

fn fetch_contains(fetch: &[ast::Spanned<ast::Idiom>], name: &str) -> bool {
    idiom_segments(fetch)
        .iter()
        .any(|segments| segments.as_slice() == [name.to_string()])
}

/// WHERE-narrowing of the projected row type (design §3.1). A SELECT's WHERE
/// is a single positive flow-guard over the result set — every returned row
/// satisfies it — so each recognized guard tightens the matching projected
/// field's kind. It only ever *tightens* a leaf already present in the
/// projection, so computed columns and fields the WHERE does not mention are
/// left at their schema kind (prove-or-fall-back-to-schema).
///
/// The whole pass is disabled — the schema shape is returned unchanged — under
/// any boundary condition where a projected field cannot be soundly keyed to a
/// plain schema row: no WHERE clause, a `GROUP BY` (rows are groups, not source
/// rows), or a non-plain-table FROM (subquery / param / graph source).
fn apply_where_narrowing(row_kind: Kind, stmt: &ast::SelectStmt) -> Kind {
    let Some(cond) = &stmt.where_clause else {
        return row_kind;
    };
    if stmt.group.is_some() {
        return row_kind;
    }
    // A plain schema-object row is required to key row fields by name.
    match stmt.from.first().map(|from| &from.node) {
        Some(ast::Expr::Table(_) | ast::Expr::RecordId { .. }) => {}
        _ => return row_kind,
    }
    narrow_row_by_facts(row_kind, stmt, &cond.node)
}

/// [`apply_where_narrowing`] answered by the [expression-fact
/// layer](crate::analyzer::facts): the `WHERE` is **one** guard, interpreted
/// once against the row it filters, and each refinement is applied wherever the
/// place it names lands in the projected row.
///
/// Two restrictions the recognizer path carried lift here, and both lift for
/// the same reason — a refinement is keyed by a [`Place`], not by a schema
/// field path that had to double as an output key:
///
/// * **`SELECT VALUE f … WHERE f != NONE`.** The row *is* the place, so the
///   refinement applies to the whole row kind rather than to a leaf of an
///   object that does not exist.
/// * **`SELECT f AS g … WHERE f != NONE`.** The projection says where `f`
///   landed, so the refinement reaches `g`. Both output names are tightened
///   when both are projected (`SELECT f, f AS g`), because both hold the same
///   value of the same row.
fn narrow_row_by_facts(row_kind: Kind, stmt: &ast::SelectStmt, cond: &ast::Expr) -> Kind {
    // The oracle borrows the row, so the interpretation is scoped: the kinds
    // are read before any of them is rewritten.
    let facts = {
        let row = ProjectedRow {
            stmt,
            kind: &row_kind,
        };
        crate::analyzer::facts::guard_of(cond, true, None).facts(&row)
    };
    if stmt.value {
        let Some(projected) = value_projection_place(stmt) else {
            return row_kind;
        };
        let mut kind = row_kind;
        for (place, refinement) in facts.iter() {
            if *place != projected {
                continue;
            }
            if let Some(narrowed) = refinement.apply(&kind) {
                kind = narrowed;
            }
        }
        return kind;
    }
    let Kind::Literal(KindLiteral::Object(mut fields)) = row_kind else {
        return row_kind;
    };
    for (place, refinement) in facts.iter() {
        if !matches!(place.root, PlaceRoot::RowField) {
            continue;
        }
        for path in output_paths(place, stmt) {
            refine_kind_at_path(&mut fields, &path, refinement);
        }
    }
    object_literal(fields)
}

/// The kinds the projected row holds, as the fact layer reads them.
///
/// A `WHERE` has no environment — there is no binding to resolve — but it does
/// have the row, and that is what a membership guard (`WHERE stage IN keywords`)
/// needs to name an element kind. A place the projection does not carry is
/// answered `None`, so an unprojected subject proves nothing rather than
/// something guessed.
struct ProjectedRow<'a> {
    stmt: &'a ast::SelectStmt,
    kind: &'a Kind,
}

impl crate::analyzer::facts::KindOracle for ProjectedRow<'_> {
    fn kind_of(&self, place: &Place) -> Option<Kind> {
        if !matches!(place.root, PlaceRoot::RowField) {
            return None;
        }
        if self.stmt.value {
            return (value_projection_place(self.stmt).as_ref() == Some(place))
                .then(|| self.kind.clone());
        }
        let Kind::Literal(KindLiteral::Object(fields)) = self.kind else {
            return None;
        };
        output_paths(place, self.stmt)
            .into_iter()
            .find_map(|path| kind_at_path(fields, &path))
    }
}

/// The row-field place a `SELECT VALUE` projects, when it projects one.
fn value_projection_place(stmt: &ast::SelectStmt) -> Option<Place> {
    let [ast::Projection::Expr { expr, .. }] = stmt.projections.as_slice() else {
        return None;
    };
    let place = crate::analyzer::facts::place_of(&expr.node)?;
    matches!(place.root, PlaceRoot::RowField).then_some(place)
}

/// Every output path in the projected row that holds this place's value: the
/// place's own field path (the key an unaliased projection lands under), plus
/// one per `AS` alias whose source expression denotes the same place.
///
/// Absent keys are skipped downstream, so a path listed here that the
/// projection does not carry costs nothing.
fn output_paths(place: &Place, stmt: &ast::SelectStmt) -> Vec<Vec<String>> {
    let mut paths: Vec<Vec<String>> = place.field_path().into_iter().collect();
    for projection in &stmt.projections {
        let ast::Projection::Expr {
            expr,
            alias: Some(alias),
        } = projection
        else {
            continue;
        };
        if crate::analyzer::facts::place_of(&expr.node).as_ref() == Some(place) {
            paths.push(vec![alias.node.clone()]);
        }
    }
    paths
}

/// The kind at `segments` in a projected object literal, if that path is
/// present: the first segment is a key of the row, the rest is
/// [`crate::kinds::project_fields`] (schema-less — the row's own shape is all
/// the oracle answers for).
fn kind_at_path(fields: &BTreeMap<String, Kind>, segments: &[String]) -> Option<Kind> {
    let (first, rest) = segments.split_first()?;
    crate::kinds::project_fields(fields.get(first)?, rest, None)
}

/// Applies a [`Refinement`] to the leaf at `segments` in a projected object
/// literal, if that exact path is present. Tighten-only: `Refinement::apply`
/// yields `None` when the claim does not tighten the leaf, and an absent key —
/// a predicate on a field the projection does not carry — is skipped.
///
/// [`Refinement`]: crate::analyzer::facts::Refinement
fn refine_kind_at_path(
    fields: &mut BTreeMap<String, Kind>,
    segments: &[String],
    refinement: &crate::analyzer::facts::Refinement,
) {
    let Some((first, rest)) = segments.split_first() else {
        return;
    };
    let Some(kind) = fields.get_mut(first) else {
        return;
    };
    if rest.is_empty() {
        if let Some(narrowed) = refinement.apply(kind) {
            *kind = narrowed;
        }
        return;
    }
    if let Kind::Literal(KindLiteral::Object(child_fields)) = kind {
        refine_kind_at_path(child_fields, rest, refinement);
    }
}

fn apply_omit(kind: Kind, omit: &[ast::Spanned<ast::Idiom>]) -> Kind {
    let Kind::Literal(KindLiteral::Object(mut fields)) = kind else {
        return kind;
    };
    for segments in idiom_segments(omit) {
        remove_kind_at_path(&mut fields, &segments);
    }
    object_literal(fields)
}

/// `FETCH` substitutes record links with the target table's object type —
/// that is what FETCH *means*. Recursion is bounded because FETCH depth is
/// explicit. Unresolvable links (no target, unknown table) stay unchanged.
fn apply_fetch(kind: Kind, fetch: &[ast::Spanned<ast::Idiom>], schema: &SchemaIndex) -> Kind {
    let Kind::Literal(KindLiteral::Object(mut fields)) = kind else {
        return kind;
    };
    for segments in idiom_segments(fetch) {
        materialize_at_path(&mut fields, &segments, schema);
    }
    object_literal(fields)
}

fn materialize_at_path(
    fields: &mut BTreeMap<String, Kind>,
    segments: &[String],
    schema: &SchemaIndex,
) {
    let Some((first, rest)) = segments.split_first() else {
        return;
    };
    let Some(kind) = fields.get_mut(first) else {
        return;
    };

    if rest.is_empty() {
        if let Some(materialized) = materialize_record_kind(kind, schema) {
            *kind = materialized;
        }
        return;
    }

    if let Kind::Literal(KindLiteral::Object(child_fields)) = kind {
        materialize_at_path(child_fields, rest, schema);
    }
}

fn materialize_record_kind(kind: &Kind, schema: &SchemaIndex) -> Option<Kind> {
    match kind {
        Kind::Record(targets) => {
            let [target] = targets.as_slice() else {
                return None;
            };
            let table = schema.tables.get(&target.to_string())?;
            (!table.fields.is_empty()).then(|| object_kind_for_all_fields(table))
        }
        Kind::Array(element, max_len) => {
            let materialized = materialize_record_kind(element, schema)?;
            Some(Kind::Array(Box::new(materialized), *max_len))
        }
        _ => None,
    }
}

fn apply_split(kind: Kind, split: &[ast::Spanned<ast::Idiom>]) -> Kind {
    let segment_lists = idiom_segments(split);
    if segment_lists.is_empty() {
        return kind;
    }

    match kind {
        Kind::Literal(KindLiteral::Object(mut fields)) => {
            for segments in &segment_lists {
                scalarize_at_path(&mut fields, segments);
            }
            object_literal(fields)
        }
        other => scalarized_kind_for_split(other),
    }
}

fn scalarize_at_path(fields: &mut BTreeMap<String, Kind>, segments: &[String]) {
    let Some((first, rest)) = segments.split_first() else {
        return;
    };
    let Some(kind) = fields.get_mut(first) else {
        return;
    };

    if rest.is_empty() {
        *kind = scalarized_kind_for_split(kind.clone());
        return;
    }

    if let Kind::Literal(KindLiteral::Object(child_fields)) = kind {
        scalarize_at_path(child_fields, rest);
    }
}

fn scalarized_kind_for_split(kind: Kind) -> Kind {
    match kind {
        Kind::Array(element, _) | Kind::Set(element, _) => *element,
        other => other,
    }
}

fn literal_limit(stmt: &ast::SelectStmt) -> Option<u64> {
    let limit = stmt.limit.as_ref()?;
    constant_row_limit(&limit.node)
}

/// The row cap a `LIMIT` expression provably imposes, folded rather than
/// matched: `LIMIT (1)` and `LIMIT 1 + 0` are the same limit as `LIMIT 1`, and
/// a literal match saw only the last of the three.
pub(crate) fn constant_row_limit(expr: &ast::Expr) -> Option<u64> {
    use crate::analyzer::facts::term::{fold, Bindings};
    match fold(expr, Bindings::NONE)? {
        crate::analyzer::facts::ConstValue::Int(value) => u64::try_from(value).ok(),
        _ => None,
    }
}

fn slice(text: &str, range: surrealguard_syntax::span::ByteRange) -> &str {
    text[range.start() as usize..range.end() as usize].trim()
}

// ---------------------------------------------------------------------------
// Shared schema-typed kind builders (used by both worlds and by mutations)
// ---------------------------------------------------------------------------

/// The closed object type of a materialized row of `table`: every declared
/// field plus the implicit record fields SurrealDB provides on every stored
/// record — `id`, and `in`/`out` on a `TYPE RELATION` edge. Those live outside
/// `table.fields` (see [`TableDef::implicit_field_kind`]) but they are part of
/// every row the engine hands back, so every materialized-row context (`SELECT
/// *`, a mutation's returned rows, a FETCH-materialized link, `$before`/
/// `$after`) must carry them.
pub(crate) fn object_kind_for_all_fields(table: &TableDef) -> Kind {
    object_kind_for_field_prefix(table, &[], true)
}

fn object_kind_for_field_prefix(table: &TableDef, prefix: &[String], implicit: bool) -> Kind {
    let mut fields = BTreeMap::new();

    for field in table.fields.values() {
        if field.path.len() <= prefix.len() || !field.path.starts_with(prefix) {
            continue;
        }

        let segment = field.path[prefix.len()].clone();
        if fields.contains_key(&segment) {
            continue;
        }

        let child_prefix: Vec<_> = prefix
            .iter()
            .cloned()
            .chain(std::iter::once(segment.clone()))
            .collect();
        let has_descendants = table.fields.values().any(|candidate| {
            candidate.path.len() > child_prefix.len() && candidate.path.starts_with(&child_prefix)
        });

        let kind = if has_descendants {
            // A nested object is not a record: only the row itself carries
            // `id`/`in`/`out`.
            object_kind_for_field_prefix(table, &child_prefix, false)
        } else {
            field.kind.clone().unwrap_or(Kind::Any)
        };
        fields.insert(segment, kind);
    }

    if implicit && prefix.is_empty() {
        // `or_insert`: an explicit `DEFINE FIELD id/in/out` always wins over
        // the implicit kind.
        for head in ["id", "in", "out"] {
            if let Some(kind) = table.implicit_field_kind(head) {
                fields.entry(head.to_string()).or_insert(kind);
            }
        }
    }

    object_literal(fields)
}

/// The type of a field path on a table: the declared kind for leaves, a
/// closed object for paths with nested field declarations, `None` when the
/// path doesn't exist on the schema.
pub(crate) fn kind_for_path(table: &TableDef, segments: &[String]) -> Option<Kind> {
    let has_descendants = table
        .fields
        .values()
        .any(|field| field.path.len() > segments.len() && field.path.starts_with(segments));

    if has_descendants {
        return Some(object_kind_for_field_prefix(table, segments, false));
    }

    if let Some(field) = table.fields.get(&segments.join(".")) {
        return Some(field.kind.clone().unwrap_or(Kind::Any));
    }

    // Fall back to the implicit record fields (`id` on any table; `in`/`out`
    // on relation edges) and record-link boundaries. When the head segment
    // resolves to a record link — a declared `record<>` field or an implicit
    // id/in/out — with trailing segments, the path crosses into the LINKED
    // table, which validates its own fields; the traversed kind is opaque
    // here (`Any`). A bare implicit field yields its own record kind.
    let (head, rest) = segments.split_first()?;
    let head_kind = table
        .fields
        .get(head)
        .and_then(|field| field.kind.clone())
        .or_else(|| table.implicit_field_kind(head));
    match head_kind {
        // A link under any number of `option`/`array`/`set` wrappers is still
        // a link: `option<record<user>>` crosses into `user` exactly as a bare
        // `record<user>` does. Resolving what lies past it needs the schema,
        // which this resolver doesn't have — `resolve_field_path` is the
        // schema-aware entry point.
        Some(kind) if crate::kinds::record_link_shape(&kind).is_some() => {
            Some(if rest.is_empty() { kind } else { Kind::Any })
        }
        // A *refined* parent carries its subfields inside its own declared kind
        // — a literal object, possibly under `option`/`array` wrappers — rather
        // than as sibling fields, so the prefix scan above never sees them.
        // Walk into the kind to resolve the remainder, which keeps
        // `cfg.theme` a `string` when `cfg` is `option<object>` refined by a
        // `DEFINE FIELD cfg.theme`. `subkind_at` returns `None` whenever the
        // step isn't provable, so an unresolvable path still reports nothing.
        Some(kind) if !rest.is_empty() => {
            let steps: Vec<crate::schema::FieldStep> = rest
                .iter()
                .map(|name| crate::schema::FieldStep::Field(name.clone()))
                .collect();
            crate::kinds::subkind_at(&kind, &steps)
        }
        _ => None,
    }
}

/// Schema-aware field-path resolver: like [`kind_for_path`], but when a path
/// segment resolves to a record link (a declared `record<T>` field or an
/// implicit `id`/`in`/`out`) and trailing segments remain, it crosses into the
/// linked table `T` and keeps resolving there — recursively, for multi-hop
/// `a.b.c`. Union links (`record<a | b>`) resolve the remainder on every
/// variant: a single common kind is used, anything else widens to `Kind::Any`.
/// An empty (`record<>`) or unknown/schemaless target, or a remainder absent on
/// a variant, also widens to `Kind::Any` — this resolver never invents a field.
/// `None` only when the head is absent from `table` and no link was crossed,
/// exactly as `kind_for_path` would report.
pub(crate) fn resolve_field_path(
    schema: &SchemaIndex,
    table: &TableDef,
    segments: &[String],
) -> Option<Kind> {
    for split in 1..segments.len() {
        let (prefix, rest) = segments.split_at(split);
        if let Some((wrappers, targets)) = record_link_targets_at(table, prefix) {
            let resolved = resolve_across_link(schema, &targets, rest);
            return Some(rewrap_link_result(&wrappers, resolved));
        }
    }
    kind_for_path(table, segments)
}

/// The linked tables when `prefix` names a record link on `table` — a declared
/// `record<...>` field (leaf) or, for a single head segment, an implicit
/// `id`/`in`/`out` — paired with the `option`/`array`/`set` wrappers around
/// the link. `None` when `prefix` is not a record link, so callers keep
/// resolving within the same table.
///
/// The wrappers matter: `option<record<user>>` and `array<record<user>>` are
/// the two most common link shapes in real schemas, and both cross into `user`
/// just as a bare `record<user>` does — but what the traversal *yields* must
/// carry the wrappers back (see [`rewrap_link_result`]).
fn record_link_targets_at(
    table: &TableDef,
    prefix: &[String],
) -> Option<(Vec<crate::kinds::KindWrapper>, Vec<surrealdb_types::Table>)> {
    if let Some(field) = table.fields.get(&prefix.join(".")) {
        return field
            .kind
            .as_ref()
            .and_then(crate::kinds::record_link_shape);
    }
    if let [head] = prefix {
        if let Some(kind) = table.implicit_field_kind(head) {
            return crate::kinds::record_link_shape(&kind);
        }
    }
    None
}

/// Re-applies a link's `option`/`array`/`set` wrappers to what the traversal
/// resolved on the far side: `option<record<user>>.name` is `option<string>`
/// (the link can be NONE, so the field access can be too) and
/// `array<record<user>>.name` is `array<string>` (field access distributes
/// over a collection of links).
///
/// `Kind::Any` is already the "not provable" answer and absorbs the wrappers:
/// `option<any>` claims no more than `any` and only clutters the rendering.
fn rewrap_link_result(wrappers: &[crate::kinds::KindWrapper], resolved: Kind) -> Kind {
    if matches!(resolved, Kind::Any) {
        return Kind::Any;
    }
    crate::kinds::rewrap_kind(wrappers, resolved)
}

/// Resolves `rest` across a record link to `targets` — the **union** of what
/// each table answers, because that is what the value is: the link points at
/// one of them, and nobody knows which.
///
/// Two shapes the old "one common kind, else `Kind::Any`" rule threw away, both
/// engine-verified on 3.2.3 against a `document.owner` declared
/// `record<account | organization>`:
///
/// * **A field only some arms declare.** `SELECT owner.username FROM document`
///   is `[{username: 'ada'}, {username: NONE}]`, and `type::of` says `'string'`
///   then `'none'` — so the read is an `option<string>`. An absent arm
///   contributes `none`, which is the answer, not an absence of one.
/// * **Arms that disagree.** Two kinds are a union of two kinds. `any` claims
///   less than either, and it is what made a union receiver unenforceable:
///   nothing can be required of an `any`.
///
/// `Kind::Any` is still the answer where nothing is provable — an empty
/// (`record<>`) or unknown target, a schemaless one (open by design), or an arm
/// that itself resolved to `any`.
pub(crate) fn resolve_across_link(
    schema: &SchemaIndex,
    targets: &[surrealdb_types::Table],
    rest: &[String],
) -> Kind {
    if targets.is_empty() {
        return Kind::Any;
    }
    let mut arms = Vec::with_capacity(targets.len());
    for target in targets {
        let Some(table) = schema.tables.get(&target.to_string()) else {
            return Kind::Any;
        };
        // A schemaless row is open: it may well carry the field, so its arm
        // proves neither a kind nor a `none`.
        if table.fields.is_empty() {
            return Kind::Any;
        }
        match resolve_field_path(schema, table, rest) {
            Some(Kind::Any) => return Kind::Any,
            Some(kind) => arms.push(kind),
            None => arms.push(Kind::None),
        }
    }
    Kind::either(arms)
}

/// Whether `segments` is *provably* absent from `table` — the question
/// [`validate_field_path`] answers by emitting, asked without emitting.
///
/// Every suppression rule that file uses is a `false` here, so the two cannot
/// drift: an opaque intermediate segment, a `record<>` with no targets, a
/// target the workspace has no `DEFINE TABLE` for, and a schemaless table all
/// prove nothing. A union link is absent only when the remainder is absent on
/// *every* arm.
///
/// This is what lets a multi-table link be checked at all. `record<dog |
/// cat>.bark` is wrong only when no arm declares `bark`; one arm that does
/// makes it legitimate polymorphic code whose value is merely NONE for the
/// others, and reporting that would be reporting a program that works.
pub(crate) fn field_path_is_absent(
    schema: &SchemaIndex,
    table: &TableDef,
    segments: &[String],
) -> bool {
    for split in 1..segments.len() {
        let (prefix, rest) = segments.split_at(split);
        if let Some((_wrappers, targets)) = record_link_targets_at(table, prefix) {
            return !targets.is_empty()
                && targets.iter().all(|target| {
                    schema
                        .tables
                        .get(&target.to_string())
                        .is_some_and(|linked| field_path_is_absent(schema, linked, rest))
                });
        }
        if field_is_opaque_boundary(table, prefix) {
            return false;
        }
    }
    // The same two escapes `check_field_path` makes before it emits: a
    // schemaless row is open by design, and a path that resolves is present.
    !table.fields.is_empty() && kind_for_path(table, segments).is_none()
}

/// Validates a (possibly link-crossing) field path against the schema, emitting
/// `code` (E1002) at the table where a segment is genuinely absent. When the
/// path crosses a record link into a single *known* table, validation continues
/// there — so `team.badfield` reports against `team`, with its `DEFINE TABLE`
/// note. A `record<>` and an unknown/dangling target (already E1001 elsewhere)
/// suppress the finding: reporting those would double-report or risk a false
/// positive.
///
/// A **union** link is reported when the remainder is absent on every one of
/// its tables — see [`field_path_is_absent`]. It names them all rather than
/// picking one, because no single one of them is the receiver.
pub(crate) fn validate_field_path(
    ctx: &mut AnalysisContext<'_>,
    table: &TableDef,
    segments: &[String],
    span: surrealguard_syntax::span::ByteRange,
    code: u16,
) {
    for split in 1..segments.len() {
        let (prefix, rest) = segments.split_at(split);
        if let Some((_wrappers, targets)) = record_link_targets_at(table, prefix) {
            match targets.as_slice() {
                [only] => {
                    if let Some(linked) = ctx.schema().tables.get(&only.to_string()) {
                        validate_field_path(ctx, linked, rest, span, code);
                    }
                }
                many => emit_absent_on_every_link_target(ctx, many, rest, span, code),
            }
            return;
        }
        // An intermediate segment that is a concrete declared field but NOT a
        // resolvable known record link is opaque: its kind is `Any`/`None`
        // (e.g. a `COMPUTED`/`VALUE` field with no `TYPE`) or a scalar, so we
        // cannot enumerate what lies past it and cannot prove the remainder
        // absent. Suppress — soundness over completeness, mirroring the
        // `record<>`/dangling/union rule. (A prefix that is a *nested-object*
        // parent, or names nothing at all, is not opaque and keeps resolving.)
        if field_is_opaque_boundary(table, prefix) {
            return;
        }
    }
    crate::analyzer::data::check_field_path(ctx, table, segments, span, code);
}

/// Checks one key of a row-context clause — OMIT, SPLIT, FETCH, GROUP BY,
/// ORDER BY — against the schema, and reports whether the path is *provably
/// absent*.
///
/// The path goes through [`validate_field_path`], the same link-crossing
/// checker the projection and (since 75899ba) every condition already use, so
/// a clause reads a path exactly as a projection of it would: `SPLIT
/// owner.ghost` is the same wrong read as `SELECT owner.ghost`. These clauses
/// were the last callers of `check_field_path`, which treats a `record<>`
/// field as an opaque boundary — it never looked past `owner`, so every
/// mistake behind a link went unreported here.
///
/// The `bool` is what keeps a clause from reporting one defect twice. Each of
/// these clauses already has a finding for a key it cannot use (1023, 1024,
/// 2017), and each is derived from the key's *resolved kind* — which, for a
/// path that does not exist, is the vacuous `none`. "SPLIT needs a collection
/// field, but `owner.ghost` is a `none`" restates the absence in the
/// vocabulary of the wrong contract; 1002 names it directly and its fix (spell
/// the field correctly) is the only one that works. So the caller stays quiet
/// when this returns `true` and lets the root cause stand alone.
fn check_clause_field_path(
    ctx: &mut AnalysisContext<'_>,
    table: &TableDef,
    segments: &[String],
    span: surrealguard_syntax::span::ByteRange,
) -> bool {
    // The predicate is `validate_field_path`'s non-emitting twin — asking it
    // first is what makes "1002 fired" answerable before the emit, and keeps
    // the suppression from ever drifting out of step with the report.
    if !field_path_is_absent(ctx.schema(), table, segments) {
        return false;
    }
    validate_field_path(ctx, table, segments, span, 1002);
    true
}

/// Emits `code` for a path read through a **multi-table** link, when the path
/// is absent on every table the link can point at.
///
/// The message names the whole union — `record<account | organization>` is what
/// the author wrote and no one of its arms is "the" receiver — and the "did you
/// mean" is drawn from the fields the arms have in common, since a suggestion
/// only one arm declares would not fix the read either.
pub(crate) fn emit_absent_on_every_link_target(
    ctx: &mut AnalysisContext<'_>,
    targets: &[surrealdb_types::Table],
    segments: &[String],
    span: surrealguard_syntax::span::ByteRange,
    code: u16,
) {
    if targets.is_empty() {
        return;
    }
    let mut tables = Vec::new();
    for target in targets {
        let Some(table) = ctx.schema().tables.get(&target.to_string()) else {
            return;
        };
        if !field_path_is_absent(ctx.schema(), table, segments) {
            return;
        }
        tables.push(table);
    }
    let common: Vec<&str> = tables
        .first()
        .map(|first| {
            first
                .fields
                .keys()
                .filter(|name| tables.iter().all(|table| table.fields.contains_key(*name)))
                .map(String::as_str)
                .collect()
        })
        .unwrap_or_default();
    let named = targets
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join(" | ");
    let path = segments.join(".");
    let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), span);
    let mut finding = surrealguard_diagnostics::catalog::finding(
        span,
        code,
        format!("`record<{named}>` has no field `{path}`"),
    );
    if let Some(nearest) = crate::suggest::closest(&path, common) {
        finding = finding.with_help(format!("did you mean `{nearest}`?"));
    }
    ctx.emit(finding);
}

/// Whether `prefix` names a concrete declared *leaf* field on `table` that is
/// not a resolvable known record link — traversing past it is unprovable, so a
/// field finding on the remainder would be unsound. A nested-object prefix (no
/// leaf field at `prefix`, only deeper declarations) and a prefix that names no
/// field at all are both *not* opaque: their absence/children are enumerable.
fn field_is_opaque_boundary(table: &TableDef, prefix: &[String]) -> bool {
    match table.fields.get(&prefix.join(".")) {
        // A link under `option`/`array`/`set` wrappers is traversable, so it
        // is not a boundary — the remainder is checked on the linked table.
        Some(field) => !field
            .kind
            .as_ref()
            .is_some_and(|kind| crate::kinds::record_link_shape(kind).is_some()),
        None => false,
    }
}

pub(crate) fn insert_kind_at_path(
    fields: &mut BTreeMap<String, Kind>,
    segments: &[String],
    kind: Kind,
) {
    let Some((first, rest)) = segments.split_first() else {
        return;
    };

    if rest.is_empty() {
        fields.insert(first.clone(), kind);
        return;
    }

    let parent = fields
        .entry(first.clone())
        .or_insert_with(|| object_literal(BTreeMap::new()));
    if let Kind::Literal(KindLiteral::Object(child_fields)) = parent {
        insert_kind_at_path(child_fields, rest, kind);
    }
}

fn remove_kind_at_path(fields: &mut BTreeMap<String, Kind>, segments: &[String]) {
    let Some((first, rest)) = segments.split_first() else {
        return;
    };

    if rest.is_empty() {
        fields.remove(first);
        return;
    }

    if let Some(Kind::Literal(KindLiteral::Object(child_fields))) = fields.get_mut(first) {
        remove_kind_at_path(child_fields, rest);
    }
}

/// SurrealDB's EXPLAIN row type. The documented fields, as a closed object —
/// SurrealDB may add fields per operator, which the closed literal
/// understates, but the known fields are far more useful downstream than a
/// bare `Kind::Object`.
fn explain_response_kind() -> Kind {
    let mut fields = BTreeMap::new();
    fields.insert("operator".into(), Kind::String);
    fields.insert("context".into(), Kind::String);
    fields.insert("attributes".into(), Kind::Object);
    fields.insert("children".into(), Kind::Array(Box::new(Kind::Any), None));
    fields.insert("metrics".into(), Kind::Object);
    fields.insert("total_rows".into(), Kind::Int);

    Kind::Array(Box::new(object_literal(fields)), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::SchemaIndex;
    use crate::statement_env::StatementEnv;
    use surrealguard_syntax::parse::{parse_source, ParsedSource};
    use surrealguard_syntax::source::SourceId;
    use surrealguard_syntax::span::{ByteRange, SourceSpan};

    use crate::expression::{ExpressionFact, ExpressionValueClass};
    use crate::schema::extract_schema;

    fn schema_from(source: &str) -> SchemaIndex {
        let parsed = parse_source(SourceId::new("schema"), source).expect("schema should parse");
        extract_schema(&[parsed]).schema
    }

    fn parse(query: &str) -> ParsedSource {
        parse_source(SourceId::new("query"), query).expect("query should parse")
    }

    fn lower_select(parsed: &ParsedSource) -> ast::SelectStmt {
        match surrealguard_syntax::lower::lower_first_statement(parsed, "SelectStatement")
            .expect("select statement exists")
            .node
        {
            ast::Statement::Select(stmt) => stmt,
            other => panic!("expected select, got {other:?}"),
        }
    }

    fn analyze(schema: &SchemaIndex, query: &str) -> Kind {
        analyze_with_env(schema, query, &StatementEnv::default())
    }

    fn analyze_with_env(schema: &SchemaIndex, query: &str, env: &StatementEnv) -> Kind {
        let parsed = parse(query);
        let stmt = lower_select(&parsed);
        let mut diagnostics: Vec<surrealguard_diagnostics::Finding> = Vec::new();
        let mut ctx = AnalysisContext::scoped(
            schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
            env.clone(),
            None,
        );
        select_response_kind(&stmt, &mut ctx)
    }

    fn diagnostics_for(
        schema: &SchemaIndex,
        query: &str,
    ) -> Vec<surrealguard_diagnostics::Finding> {
        let parsed = parse(query);
        let stmt = lower_select(&parsed);
        let mut diagnostics: Vec<surrealguard_diagnostics::Finding> = Vec::new();
        {
            let mut ctx = AnalysisContext::scoped(
                schema,
                parsed.source_id().clone(),
                parsed.text(),
                &mut diagnostics,
                StatementEnv::default(),
                None,
            );
            select_response_kind(&stmt, &mut ctx);
        }
        diagnostics
    }

    fn fires_4003(schema: &SchemaIndex, query: &str) -> bool {
        fires(schema, query, 4003)
    }

    fn fires(schema: &SchemaIndex, query: &str, code: u16) -> bool {
        diagnostics_for(schema, query)
            .iter()
            .any(|finding| finding.code().number() == code)
    }

    #[test]
    fn only_whole_table_needs_a_single_row_guarantee() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;",
        );

        // Unfiltered `FROM ONLY <table>` has no cardinality guarantee: 4003.
        assert!(fires_4003(&schema, "SELECT * FROM ONLY person;"));
        // A WHERE clause makes it the *filtered* case, which 4026 owns.
        assert!(!fires_4003(
            &schema,
            "SELECT * FROM ONLY person WHERE name = 'A';"
        ));
        // LIMIT 1 still exempts it.
        assert!(!fires_4003(&schema, "SELECT * FROM ONLY person LIMIT 1;"));
    }

    /// A schema whose `person` table carries a single-field UNIQUE index, a
    /// composite UNIQUE index and a plain (non-unique) index.
    fn indexed_person_schema() -> SchemaIndex {
        schema_from(
            "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD name ON person TYPE string;\n\
             DEFINE FIELD email ON person TYPE string;\n\
             DEFINE FIELD org ON person TYPE string;\n\
             DEFINE FIELD usr ON person TYPE string;\n\
             DEFINE INDEX email_idx ON person FIELDS email UNIQUE;\n\
             DEFINE INDEX org_usr ON person FIELDS org, usr UNIQUE;\n\
             DEFINE INDEX name_idx ON person FIELDS name;",
        )
    }

    #[test]
    fn a_filtered_only_without_a_single_row_proof_is_flagged() {
        let schema = indexed_person_schema();
        // A plain (non-unique) field filter: two matching rows make the engine
        // fail with `Expected a single result output when using the ONLY
        // keyword` — verified on 3.0.5.
        assert!(fires(
            &schema,
            "SELECT * FROM ONLY person WHERE name = 'A';",
            4026
        ));
        // A non-unique index is no proof either.
        assert!(fires(
            &schema,
            "SELECT * FROM ONLY person WHERE name != 'A';",
            4026
        ));
        // Only part of a composite UNIQUE index: verified to still error.
        assert!(fires(
            &schema,
            "SELECT * FROM ONLY person WHERE org = 'o1';",
            4026
        ));
        // A disjunction of unique equalities: verified to still error.
        assert!(fires(
            &schema,
            "SELECT * FROM ONLY person WHERE email = 'a' OR email = 'b';",
            4026
        ));
        // `LIMIT 2` does not bound it to one: verified to still error.
        assert!(fires(
            &schema,
            "SELECT * FROM ONLY person WHERE name = 'A' LIMIT 2;",
            4026
        ));
        // Field-vs-field equality pins nothing — the compared value varies
        // per row, so the UNIQUE index on `email` bounds nothing.
        assert!(fires(
            &schema,
            "SELECT * FROM ONLY person WHERE email = name;",
            4026
        ));
        // A record-id RANGE target is many records, not one.
        assert!(fires(
            &schema,
            "SELECT * FROM ONLY person:a..z WHERE name = 'A';",
            4026
        ));
    }

    #[test]
    fn every_provable_single_row_only_filter_stays_silent() {
        let schema = indexed_person_schema();
        for query in [
            // A record-id target is one row by construction.
            "SELECT * FROM ONLY person:one WHERE name = 'A';",
            // `id` is the primary key.
            "SELECT * FROM ONLY person WHERE id = person:one;",
            "SELECT * FROM ONLY person WHERE id = $wanted;",
            // A single-field UNIQUE index, fully covered by an equality.
            "SELECT * FROM ONLY person WHERE email = 'a@x.com';",
            "SELECT * FROM ONLY person WHERE email = $email AND name = 'A';",
            // A composite UNIQUE index, every field covered.
            "SELECT * FROM ONLY person WHERE org = 'o1' AND usr = 'u1';",
            // An explicit LIMIT 1.
            "SELECT * FROM ONLY person WHERE name = 'A' LIMIT 1;",
        ] {
            assert!(
                !fires(&schema, query, 4026),
                "4026 should be silent for `{query}`"
            );
        }
    }

    #[test]
    fn a_non_only_filtered_select_is_never_flagged() {
        // 4026 is about `ONLY` alone; a normal SELECT returns an array.
        let schema = indexed_person_schema();
        assert!(!fires(
            &schema,
            "SELECT * FROM person WHERE name = 'A';",
            4026
        ));
    }

    fn object_fields(kind: &Kind) -> &BTreeMap<String, Kind> {
        let Kind::Literal(KindLiteral::Object(fields)) = kind else {
            panic!("expected object literal, got {kind:?}");
        };
        fields
    }

    fn array_element(kind: &Kind) -> &Kind {
        let Kind::Array(element, _) = kind else {
            panic!("expected array kind, got {kind:?}");
        };
        element
    }

    #[test]
    fn wildcard_select_infers_array_of_known_table_fields() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD age ON person TYPE int;",
        );

        let kind = analyze(&schema, "SELECT * FROM person;");

        let Kind::Array(_, max_len) = &kind else {
            panic!("expected array kind, got {kind:?}");
        };
        assert_eq!(*max_len, None);
        let fields = object_fields(array_element(&kind));
        assert_eq!(fields["name"], Kind::String);
        assert_eq!(fields["age"], Kind::Int);
        // Every stored record has an `id`; `*` returns it (TG-1).
        assert_eq!(fields["id"], record_of("person"));
        assert_eq!(fields.len(), 3);
    }

    fn record_of(table: &str) -> Kind {
        Kind::Record(vec![surrealdb_types::Table::from(table)])
    }

    /// The row object of a SELECT's response (unwrapping the array).
    fn row_fields(schema: &SchemaIndex, query: &str) -> BTreeMap<String, Kind> {
        object_fields(array_element(&analyze(schema, query))).clone()
    }

    const RELATION_SCHEMA: &str = "DEFINE TABLE person SCHEMAFULL;\n\
         DEFINE FIELD name ON person TYPE string;\n\
         DEFINE TABLE post SCHEMAFULL;\n\
         DEFINE FIELD title ON post TYPE string;\n\
         DEFINE TABLE likes SCHEMAFULL TYPE RELATION FROM person TO post;\n\
         DEFINE FIELD since ON likes TYPE datetime;";

    #[test]
    fn wildcard_rows_on_a_relation_carry_in_and_out_with_the_endpoint_kinds() {
        let schema = schema_from(RELATION_SCHEMA);

        let fields = row_fields(&schema, "SELECT * FROM likes;");
        assert_eq!(fields["id"], record_of("likes"));
        assert_eq!(fields["in"], record_of("person"));
        assert_eq!(fields["out"], record_of("post"));
        assert_eq!(fields["since"], Kind::Datetime);
        assert_eq!(fields.len(), 4);
    }

    // ---------------------------------------------------------------------
    // TG-2: a wildcard SEEDS the row — it never swallows its siblings.
    //
    // Every expectation below was read off a live SurrealDB 3.0.5 server; the
    // JSON it returned is quoted on each test.
    // ---------------------------------------------------------------------

    const WILDCARD_SIBLING_SCHEMA: &str = "DEFINE TABLE person SCHEMAFULL;\n\
         DEFINE FIELD name ON person TYPE string;\n\
         DEFINE FIELD age ON person TYPE int;\n\
         DEFINE FIELD address ON person TYPE object;\n\
         DEFINE FIELD address.city ON person TYPE string;\n\
         DEFINE FIELD address.zip ON person TYPE string;\n\
         DEFINE TABLE account SCHEMAFULL;\n\
         DEFINE FIELD label ON account TYPE string;\n\
         DEFINE TABLE has_account SCHEMAFULL TYPE RELATION FROM person TO account;\n\
         DEFINE FIELD since ON has_account TYPE datetime;";

    /// `SELECT *, ->has_account AS acc FROM person`
    /// → `{"acc": [...], "address": {...}, "age": 30, "id": ..., "name": "A"}`
    #[test]
    fn a_graph_sibling_beside_a_wildcard_is_added_to_the_full_row() {
        let schema = schema_from(WILDCARD_SIBLING_SCHEMA);

        let fields = row_fields(&schema, "SELECT *, ->has_account AS acc FROM person;");

        assert_eq!(
            fields["acc"],
            Kind::Array(Box::new(record_of("has_account")), None)
        );
        // The wildcard's own fields all survive alongside it.
        assert_eq!(fields["name"], Kind::String);
        assert_eq!(fields["age"], Kind::Int);
        assert_eq!(fields["id"], record_of("person"));
        assert!(fields.contains_key("address"));
        assert_eq!(fields.len(), 5);
    }

    /// The no-wildcard control: the sibling alone types the row.
    /// `SELECT ->has_account AS acc FROM person` → `{"acc": [...]}`
    #[test]
    fn a_graph_projection_without_a_wildcard_is_the_whole_row() {
        let schema = schema_from(WILDCARD_SIBLING_SCHEMA);

        let fields = row_fields(&schema, "SELECT ->has_account AS acc FROM person;");

        assert_eq!(
            fields["acc"],
            Kind::Array(Box::new(record_of("has_account")), None)
        );
        assert_eq!(fields.len(), 1);
    }

    /// A bare field renamed to a bare alias REPLACES the field it renames.
    /// `SELECT *, name AS n FROM person`
    /// → `{"address": {...}, "age": 30, "id": ..., "n": "A"}` — no `name`.
    #[test]
    fn a_renamed_field_beside_a_wildcard_replaces_the_original() {
        let schema = schema_from(WILDCARD_SIBLING_SCHEMA);

        let fields = row_fields(&schema, "SELECT *, name AS n FROM person;");

        assert_eq!(fields["n"], Kind::String);
        assert!(!fields.contains_key("name"), "the renamed field is gone");
        assert_eq!(fields["age"], Kind::Int);
        assert_eq!(fields["id"], record_of("person"));
        assert_eq!(fields.len(), 4);
    }

    /// The no-wildcard control for the same rename.
    /// `SELECT name AS n FROM person` → `{"n": "A"}`
    #[test]
    fn a_renamed_field_without_a_wildcard_is_the_whole_row() {
        let schema = schema_from(WILDCARD_SIBLING_SCHEMA);

        let fields = row_fields(&schema, "SELECT name AS n FROM person;");

        assert_eq!(fields["n"], Kind::String);
        assert_eq!(fields.len(), 1);
    }

    /// Position is irrelevant — the engine applies `*` first whatever the order.
    /// `SELECT name AS n, * FROM person`
    /// → `{"address": {...}, "age": 30, "id": ..., "n": "A"}`
    #[test]
    fn a_rename_before_the_wildcard_replaces_just_the_same() {
        let schema = schema_from(WILDCARD_SIBLING_SCHEMA);

        let fields = row_fields(&schema, "SELECT name AS n, * FROM person;");

        assert_eq!(fields["n"], Kind::String);
        assert!(!fields.contains_key("name"));
        assert_eq!(fields.len(), 4);
    }

    /// An UNALIASED sibling re-projects the field under its own name: a no-op.
    /// `SELECT *, name FROM person`
    /// → `{"address": {...}, "age": 30, "id": ..., "name": "A"}`
    #[test]
    fn an_unaliased_sibling_beside_a_wildcard_changes_nothing() {
        let schema = schema_from(WILDCARD_SIBLING_SCHEMA);

        let fields = row_fields(&schema, "SELECT *, name FROM person;");

        assert_eq!(fields["name"], Kind::String);
        assert_eq!(fields, row_fields(&schema, "SELECT * FROM person;"));
    }

    /// An alias may collide with another field — the rename still removes its
    /// source and the alias overwrites the collided key.
    /// `SELECT *, age AS name FROM person` → `{"id": ..., "name": 30}`
    /// (with `address` present; `age` gone, `name` now an int).
    #[test]
    fn a_rename_onto_another_field_removes_its_source_and_overwrites() {
        let schema = schema_from(WILDCARD_SIBLING_SCHEMA);

        let fields = row_fields(&schema, "SELECT *, age AS name FROM person;");

        assert_eq!(fields["name"], Kind::Int, "the alias wins the key");
        assert!(!fields.contains_key("age"), "the rename's source is gone");
        assert_eq!(fields.len(), 3);
    }

    /// A COMPUTED sibling is additive — it is not a rename, so its operands stay.
    /// `SELECT *, string::len(name) AS l FROM person`
    /// → `{"address": {...}, "age": 30, "id": ..., "l": 1, "name": "A"}`
    #[test]
    fn a_computed_sibling_beside_a_wildcard_keeps_its_operands() {
        let schema = schema_from(WILDCARD_SIBLING_SCHEMA);

        let fields = row_fields(&schema, "SELECT *, string::len(name) AS l FROM person;");

        assert_eq!(fields["l"], Kind::Int);
        assert_eq!(fields["name"], Kind::String, "the operand is untouched");
        assert_eq!(fields.len(), 5);
    }

    /// A NESTED source is not a rename either — the parent object stays whole.
    /// `SELECT *, address.city AS c FROM person`
    /// → `{"address": {"city": "X", "zip": "1"}, ..., "c": "X"}`
    #[test]
    fn a_nested_sibling_beside_a_wildcard_keeps_its_parent_whole() {
        let schema = schema_from(WILDCARD_SIBLING_SCHEMA);

        let fields = row_fields(&schema, "SELECT *, address.city AS c FROM person;");

        assert_eq!(fields["c"], Kind::String);
        let address = object_fields(&fields["address"]);
        assert_eq!(address["city"], Kind::String);
        assert_eq!(address["zip"], Kind::String, "the sibling key survives");
        assert_eq!(fields.len(), 5);
    }

    /// A rename to the SAME name is not a rename at all — the key stays put.
    /// `SELECT *, name AS name FROM person`
    /// → `{"address": {...}, "age": 30, "id": ..., "name": "A"}`
    #[test]
    fn a_self_rename_beside_a_wildcard_is_a_no_op() {
        let schema = schema_from(WILDCARD_SIBLING_SCHEMA);

        assert_eq!(
            row_fields(&schema, "SELECT *, name AS name FROM person;"),
            row_fields(&schema, "SELECT * FROM person;")
        );
    }

    /// An unaliased DESTRUCTURE narrows the field it destructures.
    /// `SELECT *, address.{ city } FROM person`
    /// → `{"address": {"city": "X"}, "age": 30, "id": ..., "name": "A"}`
    #[test]
    fn an_unaliased_destructure_beside_a_wildcard_narrows_the_field() {
        let schema = schema_from(WILDCARD_SIBLING_SCHEMA);

        let fields = row_fields(&schema, "SELECT *, address.{ city } FROM person;");

        let address = object_fields(&fields["address"]);
        assert_eq!(address["city"], Kind::String);
        assert_eq!(address.len(), 1, "the unselected `zip` is gone");
        assert_eq!(fields.len(), 4);
    }

    /// An ALIASED destructure is additive and leaves the source whole.
    /// `SELECT *, address.{ city } AS d FROM person`
    /// → `{"address": {"city": "X", "zip": "1"}, ..., "d": {"city": "X"}}`
    #[test]
    fn an_aliased_destructure_beside_a_wildcard_keeps_the_source_whole() {
        let schema = schema_from(WILDCARD_SIBLING_SCHEMA);

        let fields = row_fields(&schema, "SELECT *, address.{ city } AS d FROM person;");

        assert_eq!(object_fields(&fields["d"]).len(), 1);
        assert_eq!(object_fields(&fields["address"]).len(), 2);
        assert_eq!(fields.len(), 5);
    }

    /// `SELECT *, * FROM person` is just `SELECT * FROM person`.
    #[test]
    fn a_repeated_wildcard_adds_nothing() {
        let schema = schema_from(WILDCARD_SIBLING_SCHEMA);

        assert_eq!(
            row_fields(&schema, "SELECT *, * FROM person;"),
            row_fields(&schema, "SELECT * FROM person;")
        );
    }

    /// Several renames in one projection list each remove their own source.
    /// `SELECT *, name AS n, age AS m FROM person`
    /// → `{"address": {...}, "id": ..., "m": 30, "n": "A"}`
    #[test]
    fn every_rename_beside_a_wildcard_removes_its_own_source() {
        let schema = schema_from(WILDCARD_SIBLING_SCHEMA);

        let fields = row_fields(&schema, "SELECT *, name AS n, age AS m FROM person;");

        assert_eq!(fields["n"], Kind::String);
        assert_eq!(fields["m"], Kind::Int);
        assert!(!fields.contains_key("name"));
        assert!(!fields.contains_key("age"));
        assert_eq!(fields.len(), 4);
    }

    /// A relation's implicit `in`/`out` are ordinary row keys: renaming them
    /// removes them like any other.
    /// `SELECT *, in AS a, out AS b FROM has_account`
    /// → `{"a": "person:p", "b": "account:a", "id": ...}`
    #[test]
    fn renaming_in_and_out_beside_a_wildcard_removes_them() {
        let schema = schema_from(WILDCARD_SIBLING_SCHEMA);

        let fields = row_fields(&schema, "SELECT *, in AS a, out AS b FROM has_account;");

        assert_eq!(fields["a"], record_of("person"));
        assert_eq!(fields["b"], record_of("account"));
        assert_eq!(fields["id"], record_of("has_account"));
        assert_eq!(fields["since"], Kind::Datetime);
        assert!(!fields.contains_key("in"));
        assert!(!fields.contains_key("out"));
        assert_eq!(fields.len(), 4);
    }

    /// OMIT runs after the wildcard and its siblings, over the RESULT keys.
    /// `SELECT *, name AS n OMIT id FROM person`
    /// → `{"address": {...}, "age": 30, "n": "A"}`
    /// `SELECT *, name AS n OMIT n FROM person`
    /// → `{"address": {...}, "age": 30, "id": ...}`
    #[test]
    fn omit_applies_to_the_keys_the_wildcard_and_its_siblings_produced() {
        let schema = schema_from(WILDCARD_SIBLING_SCHEMA);

        let fields = row_fields(&schema, "SELECT *, name AS n OMIT id FROM person;");
        assert_eq!(fields["n"], Kind::String);
        assert!(!fields.contains_key("id"));
        assert_eq!(fields.len(), 3);

        // Omitting the alias leaves neither the alias nor its renamed source.
        let fields = row_fields(&schema, "SELECT *, name AS n OMIT n FROM person;");
        assert!(!fields.contains_key("n"));
        assert!(!fields.contains_key("name"));
        assert_eq!(fields.len(), 3);
    }

    #[test]
    fn a_multi_endpoint_relation_unions_its_in_kinds() {
        let schema = schema_from(
            "DEFINE TABLE a SCHEMAFULL;\nDEFINE FIELD x ON a TYPE int;\n\
             DEFINE TABLE b SCHEMAFULL;\nDEFINE FIELD x ON b TYPE int;\n\
             DEFINE TABLE e SCHEMAFULL TYPE RELATION FROM a|b TO b;\n\
             DEFINE FIELD w ON e TYPE int;",
        );

        let fields = row_fields(&schema, "SELECT * FROM e;");
        assert_eq!(
            fields["in"],
            Kind::Record(vec![
                surrealdb_types::Table::from("a"),
                surrealdb_types::Table::from("b"),
            ])
        );
        assert_eq!(fields["out"], record_of("b"));
    }

    #[test]
    fn a_non_relation_table_gets_no_in_or_out() {
        let schema = schema_from(RELATION_SCHEMA);

        let fields = row_fields(&schema, "SELECT * FROM person;");
        assert!(!fields.contains_key("in"));
        assert!(!fields.contains_key("out"));
        assert!(fields.contains_key("id"));
    }

    #[test]
    fn a_declared_id_field_wins_over_the_implicit_kind() {
        let schema = schema_from(
            "DEFINE TABLE custom SCHEMAFULL;\n\
             DEFINE FIELD id ON custom TYPE string;\n\
             DEFINE FIELD v ON custom TYPE int;",
        );

        let fields = row_fields(&schema, "SELECT * FROM custom;");
        assert_eq!(fields["id"], Kind::String);
    }

    #[test]
    fn nested_objects_inside_a_wildcard_row_carry_no_id() {
        let schema = schema_from(
            "DEFINE TABLE nest SCHEMAFULL;\n\
             DEFINE FIELD o ON nest TYPE object;\n\
             DEFINE FIELD o.q ON nest TYPE string;",
        );

        let fields = row_fields(&schema, "SELECT * FROM nest;");
        assert_eq!(fields["id"], record_of("nest"));
        // A nested object is not a record: only the row itself has identity.
        let nested = object_fields(&fields["o"]);
        assert!(!nested.contains_key("id"), "got: {nested:?}");
        assert_eq!(nested["q"], Kind::String);
    }

    #[test]
    fn omit_id_now_removes_the_implicit_id() {
        let schema = schema_from(RELATION_SCHEMA);

        let fields = row_fields(&schema, "SELECT * OMIT id FROM person;");
        assert!(!fields.contains_key("id"), "got: {fields:?}");
        assert_eq!(fields["name"], Kind::String);

        let edge = row_fields(&schema, "SELECT * OMIT in, out FROM likes;");
        assert!(!edge.contains_key("in"), "got: {edge:?}");
        assert!(!edge.contains_key("out"), "got: {edge:?}");
        assert_eq!(edge["id"], record_of("likes"));
    }

    #[test]
    fn a_fetched_graph_target_materializes_with_its_id() {
        let schema = schema_from(RELATION_SCHEMA);

        let fields = row_fields(&schema, "SELECT ->likes->post AS p FROM person FETCH p;");
        let post = object_fields(array_element(&fields["p"]));
        assert_eq!(post["id"], record_of("post"));
        assert_eq!(post["title"], Kind::String);
    }

    #[test]
    fn a_fetched_record_link_materializes_with_its_id() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\n\
             DEFINE TABLE team SCHEMAFULL;\nDEFINE FIELD owner ON team TYPE record<person>;",
        );

        let fields = row_fields(&schema, "SELECT * FROM team FETCH owner;");
        let owner = object_fields(&fields["owner"]);
        assert_eq!(owner["id"], record_of("person"));
        assert_eq!(owner["name"], Kind::String);
    }

    // --- shapes that must NOT gain an implicit `id` -------------------------

    #[test]
    fn an_explicit_projection_that_did_not_ask_for_id_does_not_gain_one() {
        let schema = schema_from(RELATION_SCHEMA);

        let fields = row_fields(&schema, "SELECT name FROM person;");
        assert_eq!(fields.len(), 1);
        assert!(!fields.contains_key("id"), "got: {fields:?}");

        let edge = row_fields(&schema, "SELECT since FROM likes;");
        assert_eq!(edge.len(), 1);
        assert!(!edge.contains_key("in"), "got: {edge:?}");
    }

    #[test]
    fn select_value_stays_scalar_and_gains_no_id() {
        let schema = schema_from(RELATION_SCHEMA);

        assert_eq!(
            analyze(&schema, "SELECT VALUE name FROM person;"),
            Kind::Array(Box::new(Kind::String), None)
        );
    }

    #[test]
    fn grouped_wildcard_rows_carry_no_implicit_id() {
        let schema = schema_from(RELATION_SCHEMA);

        // GROUP synthesizes result rows out of group keys and accumulators;
        // they are not materialized records, so they have no identity.
        for query in [
            "SELECT * FROM person GROUP BY name;",
            "SELECT * FROM person GROUP ALL;",
        ] {
            let fields = row_fields(&schema, query);
            assert!(!fields.contains_key("id"), "{query}: {fields:?}");
        }
        let edge = row_fields(&schema, "SELECT * FROM likes GROUP BY since;");
        assert!(!edge.contains_key("id"), "got: {edge:?}");
        assert!(!edge.contains_key("in"), "got: {edge:?}");
    }

    #[test]
    fn a_wildcard_under_group_contributes_no_fields() {
        let schema = schema_from(RELATION_SCHEMA);

        // Engine-verified: `*` names no column of a grouped row. SurrealDB
        // 3.0.5 rejects the query ("expression `*` … cannot be aggregated in
        // a group"); 2.x builds grouped rows out of the non-`*` fields only,
        // so `SELECT * … GROUP BY name` returns rows with no keys at all.
        // Listing every declared field was confidently wrong — consumers
        // dereferenced fields that never exist.
        for query in [
            "SELECT * FROM person GROUP BY name;",
            "SELECT * FROM person GROUP ALL;",
        ] {
            let fields = row_fields(&schema, query);
            assert!(fields.is_empty(), "{query}: {fields:?}");
        }

        // The other projections still type the row; only `*` drops out.
        let mixed = row_fields(&schema, "SELECT *, count() FROM person GROUP BY name;");
        assert_eq!(mixed.keys().collect::<Vec<_>>(), vec!["count"]);
    }

    #[test]
    fn an_ungrouped_wildcard_still_carries_every_field_and_the_implicit_id() {
        let schema = schema_from(RELATION_SCHEMA);

        let fields = row_fields(&schema, "SELECT * FROM person;");
        assert_eq!(fields["name"], Kind::String);
        assert_eq!(fields["id"], record_of("person"));
    }

    #[test]
    fn a_wildcard_under_any_group_clause_is_an_error() {
        let schema = schema_from(RELATION_SCHEMA);

        // SurrealDB 3.0.5 rejects every one of these outright ("expression
        // `*` within in selector cannot be aggregated in a group").
        for query in [
            "SELECT * FROM person GROUP BY name;",
            "SELECT * FROM person GROUP ALL;",
            "SELECT *, count() FROM person GROUP BY name;",
            "SELECT *, count() FROM person GROUP ALL;",
        ] {
            let codes: Vec<u16> = diagnostics_for(&schema, query)
                .iter()
                .map(|finding| finding.code().number())
                .collect();
            assert!(codes.contains(&4025), "{query}: got {codes:?}");
            // 4025 replaces 4013 here: that warning's premise is a query the
            // engine *runs*, which this one isn't.
            assert!(!codes.contains(&4013), "{query}: got {codes:?}");
        }
    }

    #[test]
    fn a_group_query_without_a_wildcard_does_not_fire_4025() {
        let schema = schema_from(RELATION_SCHEMA);

        // The explicit projections 4025's help asks for — plus a bare
        // aggregate and a `t.*` idiom, both accepted by 3.0.5.
        for query in [
            "SELECT name, count() FROM person GROUP BY name;",
            "SELECT count() FROM person GROUP ALL;",
            "SELECT person.* FROM person GROUP ALL;",
            "SELECT * FROM person;",
        ] {
            let codes: Vec<u16> = diagnostics_for(&schema, query)
                .iter()
                .map(|finding| finding.code().number())
                .collect();
            assert!(!codes.contains(&4025), "{query}: got {codes:?}");
        }

        // A GROUP key that isn't projected is still the ordinary 4013.
        let codes: Vec<u16> = diagnostics_for(&schema, "SELECT count() FROM person GROUP BY name;")
            .iter()
            .map(|finding| finding.code().number())
            .collect();
        assert!(codes.contains(&4013), "got: {codes:?}");
        assert!(!codes.contains(&4025), "got: {codes:?}");

        // Projecting the key clears 4013.
        let projected: Vec<u16> =
            diagnostics_for(&schema, "SELECT name FROM person GROUP BY name;")
                .iter()
                .map(|finding| finding.code().number())
                .collect();
        assert!(!projected.contains(&4013), "got: {projected:?}");
    }

    #[test]
    fn an_aggregate_projection_row_gains_no_id() {
        let schema = schema_from(RELATION_SCHEMA);

        let fields = row_fields(&schema, "SELECT count() AS c FROM person GROUP ALL;");
        assert_eq!(fields.len(), 1);
        assert!(!fields.contains_key("id"), "got: {fields:?}");
    }

    #[test]
    fn select_from_unknown_table_is_poison() {
        let schema = SchemaIndex::default();

        assert_eq!(analyze(&schema, "SELECT * FROM ghost;"), Kind::Any);
    }

    #[test]
    fn explicit_field_projections_infer_each_field_kind() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD age ON person TYPE int;",
        );

        let kind = analyze(&schema, "SELECT name, age FROM person;");
        let fields = object_fields(array_element(&kind));
        assert_eq!(fields.len(), 2);
        assert_eq!(fields["name"], Kind::String);
        assert_eq!(fields["age"], Kind::Int);
    }

    #[test]
    fn aliased_projection_uses_alias_as_the_field_key() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;",
        );

        let kind = analyze(&schema, "SELECT name AS display_name FROM person;");
        let fields = object_fields(array_element(&kind));
        assert!(!fields.contains_key("name"));
        assert_eq!(fields["display_name"], Kind::String);
    }

    #[test]
    fn nested_field_path_projection_builds_a_nested_object_kind() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD profile.email ON person TYPE string;",
        );

        let kind = analyze(&schema, "SELECT profile.email FROM person;");
        let fields = object_fields(array_element(&kind));
        let profile = object_fields(&fields["profile"]);
        assert_eq!(profile["email"], Kind::String);
    }

    #[test]
    fn value_modifier_scalarizes_the_result_to_the_field_kind() {
        let schema =
            schema_from("DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD age ON person TYPE int;");

        let kind = analyze(&schema, "SELECT VALUE age FROM person;");

        assert_eq!(kind, Kind::Array(Box::new(Kind::Int), None));
    }

    #[test]
    fn only_modifier_skips_the_outer_array_wrapper() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;",
        );

        // `ONLY` drops the array wrapper — but the result is `option<row>`,
        // not a bare row: even a concrete record id yields NONE when the
        // record does not exist (verified on 3.0.5).
        let kind = analyze(&schema, "SELECT * FROM ONLY person:one;");
        let fields = object_fields(option_payload(&kind));
        assert_eq!(fields["name"], Kind::String);
    }

    /// The non-NONE arm of an `option<T>` (`Either([None, T])`).
    fn option_payload(kind: &Kind) -> &Kind {
        let Kind::Either(arms) = kind else {
            panic!("expected an option kind, got {kind:?}");
        };
        assert_eq!(arms.len(), 2, "expected option<T>, got {kind:?}");
        assert_eq!(arms[0], Kind::None, "expected a NONE arm in {kind:?}");
        &arms[1]
    }

    #[test]
    fn every_only_form_is_optional() {
        // No `FROM ONLY` form is statically guaranteed to produce a row: a
        // missing record id, an unmatched filter and an over-filtered
        // `LIMIT 1` all evaluate to NONE on 3.0.5.
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;",
        );
        for query in [
            "SELECT * FROM ONLY person:one;",
            "SELECT * FROM ONLY person WHERE id = person:one;",
            "SELECT * FROM ONLY person LIMIT 1;",
            "SELECT * FROM ONLY person WHERE name != 'a' LIMIT 1;",
        ] {
            let kind = analyze(&schema, query);
            let fields = object_fields(option_payload(&kind));
            assert_eq!(fields["name"], Kind::String, "for `{query}`");
        }
        // A VALUE projection is optional the same way.
        assert_eq!(
            analyze(&schema, "SELECT VALUE name FROM ONLY person:one;"),
            Kind::either(vec![Kind::None, Kind::String]),
        );
    }

    /// Analyzes `schema` + `query` as a two-source workspace and returns the
    /// kind of the query's **last** statement together with all findings.
    fn workspace_last_kind(
        schema: &str,
        query: &str,
    ) -> (Option<Kind>, Vec<surrealguard_diagnostics::Finding>) {
        let mut workspace = crate::analysis::Workspace::default();
        workspace.add_virtual_source("schema".into(), schema.into());
        let id = workspace.add_virtual_source("query".into(), query.into());
        let output = crate::analysis::analyze_workspace(&workspace);
        let last = output.sources[&id]
            .statements
            .last()
            .and_then(|statement| statement.response_kind.clone());
        (last, output.diagnostics.clone())
    }

    #[test]
    fn a_none_guard_narrows_an_only_result_back_to_a_bare_row() {
        // The other direction: after `IF $x = NONE THEN THROW … END` the
        // optionality is gone and the row's fields resolve directly.
        let (kind, findings) = workspace_last_kind(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;",
            "LET $p = SELECT name FROM ONLY person:one;\n\
             IF $p = NONE THEN THROW 'missing' END;\n\
             RETURN $p.name;",
        );
        assert_eq!(kind, Some(Kind::String));
        assert!(findings.is_empty(), "unexpected findings: {findings:?}");
    }

    #[test]
    fn limit_literal_sets_the_array_max_len() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;",
        );

        let kind = analyze(&schema, "SELECT * FROM person LIMIT 5;");

        let Kind::Array(_, max_len) = kind else {
            panic!("expected array kind");
        };
        assert_eq!(max_len, Some(5));
    }

    #[test]
    fn dynamic_comparison_projection_infers_bool() {
        let schema =
            schema_from("DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD age ON person TYPE int;");

        let kind = analyze(&schema, "SELECT age >= 18 AS adult FROM person;");
        let fields = object_fields(array_element(&kind));
        assert_eq!(fields["adult"], Kind::Bool);
    }

    #[test]
    fn unaliased_computed_projections_are_keyed_by_source_text() {
        let schema =
            schema_from("DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD age ON person TYPE int;");

        let kind = analyze(&schema, "SELECT age >= 18 FROM person;");
        let fields = object_fields(array_element(&kind));
        assert_eq!(fields["age >= 18"], Kind::Bool);
    }

    #[test]
    fn unaliased_bare_count_projection_is_keyed_count() {
        // SurrealDB names a bare `count()` projection `count`, not its source
        // text `count()`. (Other unaliased computed projections keep their
        // source text — see `unaliased_computed_projections_are_keyed_by_source_text`.)
        let schema = schema_from(
            "DEFINE TABLE account SCHEMAFULL;\nDEFINE FIELD name ON account TYPE string;",
        );

        let kind = analyze(&schema, "SELECT count() FROM account GROUP ALL;");
        let fields = object_fields(array_element(&kind));
        assert!(!fields.contains_key("count()"));
        assert_eq!(fields["count"], Kind::Int);
    }

    const KEY_SCHEMA: &str = "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD tags ON person TYPE array<string>;\nDEFINE FIELD meta ON person TYPE object;\nDEFINE FIELD meta.inner ON person TYPE object;\nDEFINE FIELD meta.inner.deep ON person TYPE string;\nDEFINE FUNCTION fn::abc($x: int) { RETURN $x + 1; };";

    #[test]
    fn an_unaliased_call_projection_is_keyed_by_the_bare_function_name() {
        // Verified against SurrealDB 3.0.5: a top-level call is named by its
        // function name, with no parens and no arguments — the *outer* one
        // when calls nest.
        let schema = schema_from(KEY_SCHEMA);

        let fields = row_fields(
            &schema,
            "SELECT string::len(name), time::now(), fn::abc(age), string::len(string::uppercase(name)) FROM person;",
        );

        assert_eq!(fields["string::len"], Kind::Int);
        assert_eq!(fields["time::now"], Kind::Datetime);
        assert_eq!(fields["fn::abc"], Kind::Int);
        assert!(!fields.contains_key("string::len(name)"), "got: {fields:?}");
        assert!(!fields.contains_key("time::now()"), "got: {fields:?}");
        assert!(!fields.contains_key("fn::abc(age)"), "got: {fields:?}");
    }

    #[test]
    fn a_projection_that_is_not_a_top_level_call_keeps_its_source_text() {
        // The rule is about the *top-level* node: a call nested inside a
        // binary/cast/container leaves the projection keyed by source text
        // (all four verified on 3.0.5).
        let schema = schema_from(KEY_SCHEMA);

        let fields = row_fields(
            &schema,
            "SELECT math::abs(age) + 1, age + 1, age > 20, <int> string::len(name) FROM person;",
        );

        assert!(fields.contains_key("math::abs(age) + 1"), "got: {fields:?}");
        assert!(fields.contains_key("age + 1"), "got: {fields:?}");
        assert!(fields.contains_key("age > 20"), "got: {fields:?}");
        assert!(
            fields.contains_key("<int> string::len(name)"),
            "got: {fields:?}"
        );
        assert!(!fields.contains_key("math::abs"), "got: {fields:?}");
    }

    #[test]
    fn an_unaliased_method_projection_drops_the_method_from_its_key() {
        // `name.len()` returns under `name`, and a multi-part path keeps its
        // nesting (`meta.inner.deep.len()` → `{meta: {inner: {deep: …}}}`).
        // Indexes, filters and splats are dropped from the key the same way.
        // All verified on 3.0.5.
        let schema = schema_from(KEY_SCHEMA);

        let fields = row_fields(&schema, "SELECT name.len() FROM person;");
        assert!(fields.contains_key("name"), "got: {fields:?}");
        assert!(!fields.contains_key("name.len()"), "got: {fields:?}");

        let nested = row_fields(&schema, "SELECT meta.inner.deep.len() FROM person;");
        let meta = object_fields(&nested["meta"]);
        let inner = object_fields(&meta["inner"]);
        assert!(inner.contains_key("deep"), "got: {nested:?}");

        // Chained methods drop together; an index part drops too.
        let chained = row_fields(
            &schema,
            "SELECT name.len().to_string(), tags[0] FROM person;",
        );
        assert!(chained.contains_key("name"), "got: {chained:?}");
        assert!(chained.contains_key("tags"), "got: {chained:?}");
        assert!(!chained.contains_key("tags[0]"), "got: {chained:?}");
    }

    #[test]
    fn a_method_projection_is_typed_by_its_receivers_method() {
        // `SELECT name.len()` is an `int` on 3.0.5, not an unknown: the
        // projection runs the same inference every other computed projection
        // gets, so method dispatch (and indexing) applies.
        let schema = schema_from(KEY_SCHEMA);

        let fields = row_fields(
            &schema,
            "SELECT name.len(), tags[0], name.uppercase(), age.to_string() FROM person;",
        );
        assert_eq!(fields["name"], Kind::String, "got: {fields:?}");
        assert_eq!(fields["tags"], Kind::String);
        assert_eq!(fields["age"], Kind::String);

        // `name.len()` alone (the reported case) is an int.
        let len = row_fields(&schema, "SELECT name.len() FROM person;");
        assert_eq!(len["name"], Kind::Int);

        // A method the receiver genuinely doesn't have stays a poison entry.
        let unknown = row_fields(&schema, "SELECT name.frobnicate() FROM person;");
        assert_eq!(unknown["name"], Kind::Any);
    }

    #[test]
    fn a_method_on_a_graph_traversal_keys_under_the_traversal_segments() {
        // `->knows->person.name.len()` returns
        // `{"->knows": {"->person": {name: …}}}` on 3.0.5 — the method drops,
        // the traversal segments and field tail stay.
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE TABLE knows TYPE RELATION IN person OUT person;",
        );

        let fields = row_fields(&schema, "SELECT ->knows->person.name.len() FROM person;");
        let knows = object_fields(&fields["->knows"]);
        let target = object_fields(&knows["->person"]);
        assert!(target.contains_key("name"), "got: {fields:?}");
    }

    #[test]
    fn a_leading_value_idiom_keeps_its_source_text_key() {
        // `$obj.a.len()` is keyed under a literal `$obj` segment by the
        // engine, a rendering we don't reproduce — those keep source text
        // rather than silently claiming a wrong path.
        let schema = schema_from(KEY_SCHEMA);

        let fields = row_fields(&schema, "SELECT $obj.a.len() FROM person;");
        assert!(fields.contains_key("$obj.a.len()"), "got: {fields:?}");
        assert!(!fields.contains_key("a"), "got: {fields:?}");
    }

    #[test]
    fn type_field_projections_are_named_by_the_fields_they_select() {
        // 3.0.5 expands `type::field`/`type::fields` into the named fields
        // themselves — `type::fields(['name', 'age'])` returns `{name, age}`.
        let schema = schema_from(KEY_SCHEMA);

        let single = row_fields(
            &schema,
            "SELECT type::field('meta.inner.deep') FROM person;",
        );
        let meta = object_fields(&single["meta"]);
        let inner = object_fields(&meta["inner"]);
        assert_eq!(inner["deep"], Kind::String);

        let many = row_fields(&schema, "SELECT type::fields(['name', 'age']) FROM person;");
        assert_eq!(many["name"], Kind::String);
        assert_eq!(many["age"], Kind::Int);

        // An alias overrides the expansion, and a non-constant path falls
        // back to the ordinary naming (the runtime key is unknowable).
        let aliased = row_fields(&schema, "SELECT type::field('name') AS n FROM person;");
        assert!(aliased.contains_key("n"), "got: {aliased:?}");
        let dynamic = row_fields(&schema, "SELECT type::field($path) FROM person;");
        assert!(dynamic.contains_key("type::field"), "got: {dynamic:?}");
    }

    #[test]
    fn grouped_projection_types_key_fields_and_aggregates() {
        // GROUP BY must not degrade the result type: a grouped row is the group
        // key fields plus the aggregate projections, keyed like an ungrouped
        // projection (`count()` → `count`).
        let schema = schema_from(
            "DEFINE TABLE employee_of SCHEMAFULL;\nDEFINE FIELD status ON employee_of TYPE string;",
        );

        let kind = analyze(
            &schema,
            "SELECT status, count() FROM employee_of GROUP BY status;",
        );
        let fields = object_fields(array_element(&kind));
        assert_eq!(fields["status"], Kind::String);
        assert_eq!(fields["count"], Kind::Int);

        // GROUP ALL keeps the array wrapper and the aggregate's type.
        let total = analyze(&schema, "SELECT count() FROM employee_of GROUP ALL;");
        assert_eq!(
            total,
            Kind::Array(
                Box::new(object_literal(
                    [("count".to_string(), Kind::Int)].into_iter().collect()
                )),
                None
            )
        );
    }

    #[test]
    fn dynamic_projection_resolves_let_bound_variable_kind_against_known_table() {
        let schema =
            schema_from("DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD age ON person TYPE int;");
        let mut env = StatementEnv::default();
        env.define_let(
            "bonus".into(),
            ExpressionFact::new(
                SourceSpan::new(SourceId::new("env"), ByteRange::new(0, 1).unwrap()),
                ExpressionValueClass::Literal,
            )
            .with_kind(Kind::Int),
        );

        let kind = analyze_with_env(&schema, "SELECT $bonus + 1 AS total FROM person;", &env);
        let fields = object_fields(array_element(&kind));
        assert_eq!(fields["total"], Kind::Int);
    }

    #[test]
    fn wildcard_select_resolves_graph_traversal_target_table() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE TABLE post SCHEMAFULL;\nDEFINE FIELD title ON post TYPE string;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;",
        );

        let kind = analyze(&schema, "SELECT * FROM person->likes->post;");
        let fields = object_fields(array_element(&kind));
        assert_eq!(fields["title"], Kind::String);
    }

    #[test]
    fn a_wildcard_after_a_traversal_expands_the_landed_row() {
        // The gap this test exists for: `->follows->user.*` inferred `any`,
        // silently, while `->follows->user.name` after the same hops was
        // right. The wildcard must expand to the target's field object —
        // exactly what `SELECT * FROM user` projects. Engine-verified on
        // 3.0.5: `SELECT ->follows->user.* AS r FROM ONLY user:ada` ->
        // `{r: [{age, id, name}]}`.
        let schema = schema_from(concat!(
            "DEFINE TABLE user SCHEMAFULL;\n",
            "DEFINE FIELD name ON user TYPE string;\n",
            "DEFINE FIELD age ON user TYPE int;\n",
            "DEFINE TABLE follows TYPE RELATION IN user OUT user SCHEMAFULL;\n",
            "DEFINE FIELD since ON follows TYPE datetime;\n",
        ));

        let target = analyze(&schema, "SELECT ->follows->user.* AS r FROM user;");
        let row = object_fields(array_element(&target));
        let user = object_fields(array_element(&row["r"]));
        assert_eq!(user["name"], Kind::String);
        assert_eq!(user["age"], Kind::Int);
        assert_eq!(user["id"], Kind::Record(vec!["user".into()]));

        // A single hop's wildcard stands on the EDGE, just as `->follows.since`
        // does — including the implicit `id`/`in`/`out`.
        let edge = analyze(&schema, "SELECT ->follows.* AS r FROM user;");
        let row = object_fields(array_element(&edge));
        let follows = object_fields(array_element(&row["r"]));
        assert_eq!(follows["since"], Kind::Datetime);
        assert_eq!(follows["in"], Kind::Record(vec!["user".into()]));
        assert_eq!(follows["out"], Kind::Record(vec!["user".into()]));

        // `.*` names no path segment: a field after it reads off the row, and
        // the unaliased key drops the wildcard the way the engine's own key
        // simplification does.
        let tail = analyze(&schema, "SELECT ->follows->user.*.name FROM user;");
        let row = object_fields(array_element(&tail));
        let follows = object_fields(&row["->follows"]);
        let user = object_fields(&follows["->user"]);
        assert_eq!(user["name"], Kind::Array(Box::new(Kind::String), None));
    }

    #[test]
    fn graph_projection_without_alias_nests_under_traversal_segments() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string;\nDEFINE FIELD age ON user TYPE int;\nDEFINE TABLE friend TYPE RELATION IN person OUT user;",
        );

        let kind = analyze(&schema, "SELECT ->friend->user.{name, age} FROM person;");
        let fields = object_fields(array_element(&kind));

        let friend = object_fields(&fields["->friend"]);
        let selected = object_fields(array_element(&friend["->user"]));
        assert_eq!(selected["name"], Kind::String);
        assert_eq!(selected["age"], Kind::Int);
    }

    #[test]
    fn unaliased_graph_targets_nest_under_arrow_keys_as_record_arrays() {
        // `SELECT ->friend->user FROM person` defaults to
        // `{ "->friend": { "->user": array<record<user>> } }` per row.
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string;\nDEFINE TABLE friend TYPE RELATION IN person OUT user;",
        );

        let kind = analyze(&schema, "SELECT ->friend->user FROM person;");
        let fields = object_fields(array_element(&kind));

        let friend = object_fields(&fields["->friend"]);
        let Kind::Array(element, _) = &friend["->user"] else {
            panic!("expected traversal array, got {:?}", friend["->user"]);
        };
        assert_eq!(**element, Kind::Record(vec!["user".into()]));
    }

    #[test]
    fn aliased_graph_destructure_projects_an_object_under_the_alias() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string;\nDEFINE TABLE friend TYPE RELATION IN person OUT user;",
        );

        let kind = analyze(
            &schema,
            "SELECT ->friend->user.{name} AS friends FROM person;",
        );
        let fields = object_fields(array_element(&kind));

        let friends = array_element(&fields["friends"]);
        let selected = object_fields(friends);
        assert_eq!(selected["name"], Kind::String);
    }

    #[test]
    fn explain_modifier_yields_the_fixed_explain_kind() {
        let schema = SchemaIndex::default();

        let kind = analyze(&schema, "SELECT * FROM person EXPLAIN;");
        let fields = object_fields(array_element(&kind));
        assert_eq!(fields["total_rows"], Kind::Int);
        assert_eq!(fields["operator"], Kind::String);
    }

    #[test]
    fn omit_clause_removes_the_field_from_the_object_kind() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD age ON person TYPE int;",
        );

        let kind = analyze(&schema, "SELECT * OMIT age FROM person;");
        let fields = object_fields(array_element(&kind));
        assert!(fields.contains_key("name"));
        assert!(!fields.contains_key("age"));
    }

    #[test]
    fn fetch_substitutes_record_links_with_the_target_object_type() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD best_friend ON person TYPE record<person>;",
        );

        let kind = analyze(&schema, "SELECT * FROM person FETCH best_friend;");
        let fields = object_fields(array_element(&kind));

        // Without FETCH the field is a record link; with FETCH it is the
        // target table's object type (one level deep — the nested
        // best_friend link inside stays a record).
        let friend = object_fields(&fields["best_friend"]);
        assert_eq!(friend["name"], Kind::String);
        assert!(matches!(friend["best_friend"], Kind::Record(_)));
    }

    #[test]
    fn single_hop_graph_field_projects_off_the_edge_table() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE TABLE post SCHEMAFULL;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD strength ON likes TYPE float;",
        );

        let kind = analyze(&schema, "SELECT ->likes.strength FROM person;");
        let fields = object_fields(array_element(&kind));

        let likes = object_fields(&fields["->likes"]);
        assert_eq!(likes["strength"], Kind::Array(Box::new(Kind::Float), None));
    }

    #[test]
    fn value_graph_projection_yields_the_traversal_array() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE TABLE post SCHEMAFULL;\nDEFINE FIELD title ON post TYPE string;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;",
        );

        let kind = analyze(&schema, "SELECT VALUE ->likes->post.title FROM person;");

        assert_eq!(
            kind,
            Kind::Array(Box::new(Kind::Array(Box::new(Kind::String), None)), None)
        );
    }

    #[test]
    fn aliased_graph_target_materializes_when_fetched_by_alias() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string;\nDEFINE TABLE friend TYPE RELATION IN person OUT user;",
        );

        let fetched = analyze(
            &schema,
            "SELECT ->friend->user AS friends FROM person FETCH friends;",
        );
        let fields = object_fields(array_element(&fetched));
        let friends = array_element(&fields["friends"]);
        let user = object_fields(friends);
        assert_eq!(user["name"], Kind::String);

        // Without the FETCH the alias stays a record link array.
        let unfetched = analyze(&schema, "SELECT ->friend->user AS friends FROM person;");
        let fields = object_fields(array_element(&unfetched));
        assert!(matches!(array_element(&fields["friends"]), Kind::Record(_)));
    }

    #[test]
    fn split_scalarizes_the_named_array_field() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD tags ON person TYPE array;",
        );

        let kind = analyze(&schema, "SELECT * FROM person SPLIT tags;");
        let fields = object_fields(array_element(&kind));

        // `array` (untargeted) scalarizes to its element kind — Any here.
        assert_eq!(fields["tags"], Kind::Any);
        assert_eq!(fields["name"], Kind::String);
    }

    #[test]
    fn fetch_resolves_across_a_record_link_to_judge_the_target_field() {
        // `team` on `user` is a record link; the FETCH check must cross it to
        // type the trailing segment. `team.label` is a scalar (FETCH does
        // nothing → 1023); `team.owner` is itself a link (FETCH is meaningful
        // → no finding). Before link-crossing both stayed `Any` and neither
        // fired.
        let schema = schema_from(
            "DEFINE TABLE team SCHEMAFULL;\n\
             DEFINE FIELD label ON team TYPE string;\n\
             DEFINE FIELD owner ON team TYPE record<user>;\n\
             DEFINE TABLE user SCHEMAFULL;\n\
             DEFINE FIELD team ON user TYPE record<team>;",
        );

        let (_, scalar) = analyze_diagnostics(&schema, "SELECT * FROM user FETCH team.label;");
        assert!(
            codes(&scalar).contains(&1023),
            "FETCH over a linked scalar should fire 1023, got {:?}",
            codes(&scalar)
        );

        let (_, linked) = analyze_diagnostics(&schema, "SELECT * FROM user FETCH team.owner;");
        assert!(
            !codes(&linked).contains(&1023),
            "FETCH over a linked record must not fire 1023, got {:?}",
            codes(&linked)
        );
    }

    #[test]
    fn omit_removes_nested_paths_from_nested_object_kinds() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD profile.email ON person TYPE string;\nDEFINE FIELD profile.city ON person TYPE string;",
        );

        let kind = analyze(&schema, "SELECT * OMIT profile.email FROM person;");
        let fields = object_fields(array_element(&kind));
        let profile = object_fields(&fields["profile"]);

        assert!(profile.contains_key("city"));
        assert!(!profile.contains_key("email"));
    }

    #[test]
    fn type_irrelevant_clauses_do_not_affect_the_response_kind() {
        // The permissive grammar allows RETURN on SELECT; it cannot change
        // what the SELECT produces, so the shape is inferred normally.
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;",
        );

        let kind = analyze(&schema, "SELECT * FROM person RETURN NONE;");
        let fields = object_fields(array_element(&kind));
        assert_eq!(fields["name"], Kind::String);
    }

    #[test]
    fn subquery_sources_type_rows_from_the_inner_response() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD age ON person TYPE int;",
        );

        let kind = analyze(&schema, "SELECT * FROM (SELECT name FROM person);");
        let fields = object_fields(array_element(&kind));

        assert_eq!(fields.len(), 1);
        assert_eq!(fields["name"], Kind::String);
    }

    #[test]
    fn param_sources_are_poison() {
        let schema = SchemaIndex::default();

        assert_eq!(analyze(&schema, "SELECT * FROM $tbl;"), Kind::Any);
    }

    /// Analyzes a SELECT and returns both its response kind and every
    /// finding it emitted.
    fn analyze_diagnostics(
        schema: &SchemaIndex,
        query: &str,
    ) -> (Kind, Vec<surrealguard_diagnostics::Finding>) {
        let parsed = parse(query);
        let stmt = lower_select(&parsed);
        let mut diagnostics: Vec<surrealguard_diagnostics::Finding> = Vec::new();
        let mut ctx = AnalysisContext::scoped(
            schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
            StatementEnv::default(),
            None,
        );
        let kind = select_response_kind(&stmt, &mut ctx);
        (kind, diagnostics)
    }

    fn codes(diagnostics: &[surrealguard_diagnostics::Finding]) -> Vec<u16> {
        diagnostics.iter().map(|d| d.code().number()).collect()
    }

    #[test]
    fn bare_count_without_group_warns_that_it_counts_per_row() {
        let schema = schema_from(
            "DEFINE TABLE employee_of SCHEMAFULL;\nDEFINE FIELD status ON employee_of TYPE string;",
        );

        let (_, diagnostics) = analyze_diagnostics(
            &schema,
            "SELECT count() FROM employee_of WHERE status = 'active';",
        );
        assert!(
            codes(&diagnostics).contains(&4023),
            "expected 4023, got {:?}",
            codes(&diagnostics)
        );
    }

    #[test]
    fn bare_count_with_group_all_is_a_real_aggregate() {
        let schema = schema_from(
            "DEFINE TABLE employee_of SCHEMAFULL;\nDEFINE FIELD status ON employee_of TYPE string;",
        );

        let (_, diagnostics) = analyze_diagnostics(
            &schema,
            "SELECT count() FROM employee_of WHERE status = 'active' GROUP ALL;",
        );
        assert!(
            !codes(&diagnostics).contains(&4023),
            "GROUP ALL count() is a total, not per-row"
        );
    }

    #[test]
    fn aggregate_over_scalar_column_infers_number_without_argument_finding() {
        let schema = schema_from(
            "DEFINE TABLE file SCHEMAFULL;\nDEFINE FIELD size_bytes ON file TYPE int;\nDEFINE FIELD status ON file TYPE string;",
        );

        let (kind, diagnostics) = analyze_diagnostics(
            &schema,
            "SELECT VALUE math::sum(size_bytes) FROM file WHERE status = 'active' GROUP ALL;",
        );
        // The column is collected into `array<int>`, so the `array`
        // argument contract holds — no 5002 false positive.
        assert!(
            !codes(&diagnostics).contains(&5002),
            "aggregate over a scalar column should not trip the argument check: {:?}",
            codes(&diagnostics)
        );
        // `array<int>` in, `int` out — engine-verified: `math::sum([1,2,3])`
        // is `6`, not `6f`.
        assert_eq!(kind, Kind::Array(Box::new(Kind::Int), None));

        // Without a GROUP clause there is no column to collect: the call
        // runs per row on one `int`, which is 4028 — and only 4028, not a
        // second report against the signature.
        let (_, ungrouped) = analyze_diagnostics(
            &schema,
            "SELECT VALUE math::sum(size_bytes) FROM file WHERE status = 'active';",
        );
        assert_eq!(
            codes(&ungrouped).iter().filter(|c| **c == 4028).count(),
            1,
            "{:?}",
            codes(&ungrouped)
        );
        assert!(
            !codes(&ungrouped).contains(&5002),
            "{:?}",
            codes(&ungrouped)
        );
    }

    #[test]
    fn collector_aggregates_promote_their_column_like_the_math_reducers() {
        // Engine-verified on 3.0.5: under a GROUP clause the aggregator hands
        // `array::distinct`/`array::group` the *collected* column, so
        // `array::distinct(name)` over a `string` column returns
        // `["ann", "bob"]` — the `array` argument contract is satisfied by the
        // collection, exactly as it is for `math::sum`. Before this the two
        // disagreed: `array::distinct` raised 5002 while `array::group` was
        // waved through with `any`.
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD name ON person TYPE string;\n\
             DEFINE FIELD tier ON person TYPE string;",
        );

        for call in ["array::distinct(name)", "array::group(name)"] {
            let (kind, diagnostics) = analyze_diagnostics(
                &schema,
                &format!("SELECT tier, {call} AS names FROM person GROUP BY tier;"),
            );
            assert!(
                !codes(&diagnostics).contains(&5002),
                "`{call}` under GROUP BY receives the collected column: {:?}",
                codes(&diagnostics)
            );
            let fields = object_fields(array_element(&kind));
            // And the collected element kind is knowable — not `any`.
            assert_eq!(
                fields["names"],
                Kind::Array(Box::new(Kind::String), None),
                "`{call}` should collect `string`s"
            );
            assert_eq!(fields["tier"], Kind::String);
        }
    }

    #[test]
    fn a_non_aggregate_array_function_keeps_its_per_row_contract() {
        // The promoted set is the engine's, not "every `array::` function":
        // `array::len(name)` is evaluated per row (3.0.5 returns one result per
        // row, not a reduction), so its `array` contract still applies to the
        // per-row `string` — and still reports.
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD name ON person TYPE string;\n\
             DEFINE FIELD tier ON person TYPE string;",
        );

        let (_, diagnostics) = analyze_diagnostics(
            &schema,
            "SELECT tier, array::len(name) AS n FROM person GROUP BY tier;",
        );
        assert!(
            codes(&diagnostics).contains(&5002),
            "a per-row `array::len` over a string column still violates its contract: {:?}",
            codes(&diagnostics)
        );
    }

    #[test]
    fn aggregate_promotion_leaves_already_collection_columns_alone() {
        // `scores` is already `array<int>`; the ordinary element-wise
        // reading fits, so the promotion must not fire (and no 5002).
        let schema = schema_from(
            "DEFINE TABLE team SCHEMAFULL;\nDEFINE FIELD scores ON team TYPE array<int>;",
        );

        let (_, diagnostics) =
            analyze_diagnostics(&schema, "SELECT VALUE math::sum(scores) FROM team;");
        assert!(
            !codes(&diagnostics).contains(&5002),
            "sum over an array column is already well-typed: {:?}",
            codes(&diagnostics)
        );
    }

    // -----------------------------------------------------------------------
    // Record-link field traversal in projections (`team.label`)
    // -----------------------------------------------------------------------

    fn user_with_team_schema() -> SchemaIndex {
        schema_from(
            "DEFINE TABLE team SCHEMAFULL;\n\
             DEFINE FIELD label ON team TYPE string;\n\
             DEFINE TABLE user SCHEMAFULL;\n\
             DEFINE FIELD name ON user TYPE string;\n\
             DEFINE FIELD team ON user TYPE record<team>;",
        )
    }

    #[test]
    fn record_link_field_traversal_resolves_the_linked_field_kind() {
        let schema = user_with_team_schema();

        // `team` is `record<team>`; `.label` crosses into `team` and resolves
        // to the linked field's kind, nested under `team`.
        let kind = analyze(&schema, "SELECT team.label FROM user;");
        let fields = object_fields(array_element(&kind));
        let team = object_fields(&fields["team"]);
        assert_eq!(team["label"], Kind::String);
    }

    #[test]
    fn record_link_traversal_to_absent_field_emits_1002_at_linked_table() {
        let schema = user_with_team_schema();

        let (_, diagnostics) = analyze_diagnostics(&schema, "SELECT team.badfield FROM user;");
        let finding = diagnostics
            .iter()
            .find(|f| f.code().number() == 1002)
            .expect("expected 1002 for the absent linked field");
        // The message names the *linked* table, and a related note points at
        // `team`'s definition.
        assert!(
            finding.message().contains("`team` has no field `badfield`"),
            "unexpected message: {}",
            finding.message()
        );
        assert!(
            !finding.related().is_empty(),
            "expected a `defined here` note at team's DEFINE TABLE"
        );
    }

    #[test]
    fn multi_hop_record_link_traversal_resolves_the_leaf_kind() {
        let schema = schema_from(
            "DEFINE TABLE c SCHEMAFULL;\n\
             DEFINE FIELD label ON c TYPE string;\n\
             DEFINE TABLE b SCHEMAFULL;\n\
             DEFINE FIELD c ON b TYPE record<c>;\n\
             DEFINE TABLE a SCHEMAFULL;\n\
             DEFINE FIELD b ON a TYPE record<b>;",
        );

        // `a.b.c.label` hops a -> b -> c, then reads `label`.
        let kind = analyze(&schema, "SELECT VALUE b.c.label FROM a;");
        assert_eq!(kind, Kind::Array(Box::new(Kind::String), None));
    }

    #[test]
    fn union_record_link_reads_the_union_of_what_its_tables_answer() {
        let schema = schema_from(
            "DEFINE TABLE cat SCHEMAFULL;\n\
             DEFINE FIELD legs ON cat TYPE int;\n\
             DEFINE FIELD purrs ON cat TYPE bool;\n\
             DEFINE TABLE dog SCHEMAFULL;\n\
             DEFINE FIELD legs ON dog TYPE int;\n\
             DEFINE TABLE owner SCHEMAFULL;\n\
             DEFINE FIELD pet ON owner TYPE record<cat | dog>;",
        );

        // `legs` exists on both with a common kind -> int.
        let kind = analyze(&schema, "SELECT VALUE pet.legs FROM owner;");
        assert_eq!(kind, Kind::Array(Box::new(Kind::Int), None));

        // `purrs` exists only on `cat`. A `dog` answers NONE for it — verified
        // on 3.2.3, where `type::of(owner.username)` over a two-table link is
        // `'string'` for the arm that has it and `'none'` for the arm that does
        // not — so the read is an `option<bool>`, and it is not a finding:
        // one arm declaring the field makes the query legitimate.
        let (kind, diagnostics) =
            analyze_diagnostics(&schema, "SELECT VALUE pet.purrs FROM owner;");
        assert_eq!(
            kind,
            Kind::Array(Box::new(Kind::either(vec![Kind::Bool, Kind::None])), None)
        );
        assert!(
            !codes(&diagnostics).contains(&1002),
            "a field one arm declares must not emit 1002: {:?}",
            codes(&diagnostics)
        );

        // Absent on EVERY arm is the case that is provable, and it reports.
        let (_, diagnostics) = analyze_diagnostics(&schema, "SELECT VALUE pet.nope FROM owner;");
        assert!(
            codes(&diagnostics).contains(&1002),
            "a field no arm declares must emit 1002: {:?}",
            codes(&diagnostics)
        );

        // A schemaless arm is open by design and vouches for nothing, so the
        // whole read stays unchecked and untyped.
        let lenient = schema_from(
            "DEFINE TABLE cat SCHEMAFULL;\n\
             DEFINE FIELD legs ON cat TYPE int;\n\
             DEFINE TABLE dog;\n\
             DEFINE TABLE owner SCHEMAFULL;\n\
             DEFINE FIELD pet ON owner TYPE record<cat | dog>;",
        );
        let (kind, diagnostics) =
            analyze_diagnostics(&lenient, "SELECT VALUE pet.nope FROM owner;");
        assert_eq!(kind, Kind::Array(Box::new(Kind::Any), None));
        assert!(
            !codes(&diagnostics).contains(&1002),
            "a schemaless arm must suppress the finding: {:?}",
            codes(&diagnostics)
        );
    }

    #[test]
    fn dangling_record_link_traversal_emits_no_1002() {
        // `team` links to a table absent from the schema — that is E1001's job,
        // not a new E1002 field finding.
        let schema = schema_from(
            "DEFINE TABLE user SCHEMAFULL;\n\
             DEFINE FIELD team ON user TYPE record<ghost>;",
        );

        let (_, diagnostics) = analyze_diagnostics(&schema, "SELECT team.label FROM user;");
        assert!(
            !codes(&diagnostics).contains(&1002),
            "a dangling record link must not emit 1002: {:?}",
            codes(&diagnostics)
        );
    }

    #[test]
    fn fetch_still_expands_a_record_link_field() {
        // Capability #1 must not disturb FETCH: a bare link projection still
        // materializes to the target object type under FETCH.
        let schema = user_with_team_schema();

        let kind = analyze(&schema, "SELECT team FROM user FETCH team;");
        let fields = object_fields(array_element(&kind));
        let team = object_fields(&fields["team"]);
        assert_eq!(team["label"], Kind::String);
    }

    // -----------------------------------------------------------------------
    // `.{…}` destructure field validation
    // -----------------------------------------------------------------------

    fn person_friend_user_schema() -> SchemaIndex {
        schema_from(
            "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD name ON person TYPE string;\n\
             DEFINE TABLE user SCHEMAFULL;\n\
             DEFINE FIELD name ON user TYPE string;\n\
             DEFINE FIELD age ON user TYPE int;\n\
             DEFINE TABLE friend TYPE RELATION IN person OUT user;",
        )
    }

    /// A destructure names no output key of its own: the selected OBJECT
    /// lands whole at the traversal's key, one per traversed record.
    /// `SELECT ->friend->user.{name, age} FROM person`
    /// → `{"->friend": {"->user": [{"name": …, "age": …}]}}` (3.0.5 live).
    #[test]
    fn graph_destructure_types_each_selected_field() {
        let schema = person_friend_user_schema();

        let (kind, diagnostics) =
            analyze_diagnostics(&schema, "SELECT ->friend->user.{name, age} FROM person;");
        let fields = object_fields(array_element(&kind));
        let friend = object_fields(&fields["->friend"]);
        let selected = object_fields(array_element(&friend["->user"]));
        assert_eq!(selected["name"], Kind::String);
        assert_eq!(selected["age"], Kind::Int);
        assert!(
            !codes(&diagnostics).contains(&1002),
            "a valid destructure must not emit 1002: {:?}",
            codes(&diagnostics)
        );
    }

    #[test]
    fn graph_destructure_absent_field_emits_1002_and_still_projects_valid_fields() {
        let schema = person_friend_user_schema();

        let (kind, diagnostics) =
            analyze_diagnostics(&schema, "SELECT ->friend->user.{name, aeg} FROM person;");
        // `name` still projects.
        let fields = object_fields(array_element(&kind));
        let friend = object_fields(&fields["->friend"]);
        let selected = object_fields(array_element(&friend["->user"]));
        assert_eq!(selected["name"], Kind::String);

        // `aeg` is absent on `user` -> 1002 naming `user`, with its note.
        let finding = diagnostics
            .iter()
            .find(|f| f.code().number() == 1002)
            .expect("expected 1002 for the absent destructure field");
        assert!(
            finding.message().contains("`user` has no field `aeg`"),
            "unexpected message: {}",
            finding.message()
        );
        assert!(
            !finding.related().is_empty(),
            "expected user's definition note"
        );
    }

    // -----------------------------------------------------------------------
    // A `.{…}` with something behind it
    //
    // A destructure is not a path segment either: it *replaces* the value with
    // an object of its own, and everything after it reads out of that object.
    // Every shape below is engine-verified on SurrealDB 3.0.5.
    // -----------------------------------------------------------------------

    /// `SELECT ->follows->user.{name}.name AS r FROM user` -> `{r: ['Grace']}`.
    #[test]
    fn a_field_after_a_graph_destructure_reads_the_selected_key() {
        let schema = person_friend_user_schema();

        let (kind, diagnostics) = analyze_diagnostics(
            &schema,
            "SELECT ->friend->user.{name, age}.age AS r FROM person;",
        );
        let fields = object_fields(array_element(&kind));

        assert_eq!(fields["r"], Kind::Array(Box::new(Kind::Int), None));
        assert!(
            !codes(&diagnostics).contains(&1002),
            "a selected key reads back out cleanly: {:?}",
            codes(&diagnostics)
        );
    }

    /// The destructure names no output key, the field behind it does: the
    /// engine keys `->friend->user.{name}.name` under `->friend.->user.name`.
    #[test]
    fn an_unaliased_field_after_a_graph_destructure_keys_under_the_field() {
        let schema = person_friend_user_schema();

        let kind = analyze(&schema, "SELECT ->friend->user.{name}.name FROM person;");
        let fields = object_fields(array_element(&kind));
        let friend = object_fields(&fields["->friend"]);
        let user = object_fields(&friend["->user"]);

        assert_eq!(user["name"], Kind::Array(Box::new(Kind::String), None));
    }

    /// A single hop with a tail reads off the EDGE, and a destructure there is
    /// no different: `SELECT ->follows.{since}.since AS s FROM user`
    /// -> `{s: ['2020-01-01T00:00:00Z']}`.
    #[test]
    fn a_destructure_on_the_edge_takes_a_field_tail_too() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD name ON person TYPE string;\n\
             DEFINE TABLE user SCHEMAFULL;\n\
             DEFINE FIELD name ON user TYPE string;\n\
             DEFINE TABLE friend TYPE RELATION IN person OUT user;\n\
             DEFINE FIELD since ON friend TYPE datetime;",
        );

        let kind = analyze(&schema, "SELECT ->friend.{since}.since AS s FROM person;");
        let fields = object_fields(array_element(&kind));

        assert_eq!(fields["s"], Kind::Array(Box::new(Kind::Datetime), None));
    }

    /// A field the selection does not hold is *provably* absent — there is no
    /// schema left to consult. The engine returns NONE, and the site would
    /// otherwise be a silent `any`.
    #[test]
    fn a_field_the_destructure_did_not_select_is_reported() {
        let schema = person_friend_user_schema();

        let (_, diagnostics) = analyze_diagnostics(
            &schema,
            "SELECT ->friend->user.{name}.age AS r FROM person;",
        );

        let finding = diagnostics
            .iter()
            .find(|finding| finding.code().number() == 1002)
            .expect("expected 1002 for a key the destructure did not select");
        assert!(
            finding.message().contains("`age`") && finding.message().contains("`name`"),
            "the message must name both the read and the selection: {}",
            finding.message()
        );
    }

    /// The same contract off a record link, where the destructure is not
    /// behind a traversal: `SELECT author.{name}.age FROM post` -> `null`.
    #[test]
    fn a_field_the_row_destructure_did_not_select_is_reported() {
        let schema = user_with_team_schema();

        let (_, diagnostics) =
            analyze_diagnostics(&schema, "SELECT team.{label}.nope AS r FROM user;");

        assert!(
            codes(&diagnostics).contains(&1002),
            "expected 1002 for a key the destructure did not select: {:?}",
            codes(&diagnostics)
        );
    }

    /// A destructure whose selection is wrong is reported whether or not it is
    /// the final part — it was silent purely because something followed it.
    #[test]
    fn a_destructure_with_a_tail_still_validates_its_selection() {
        let schema = person_friend_user_schema();

        let (_, graph) =
            analyze_diagnostics(&schema, "SELECT ->friend->user.{aeg}.aeg AS r FROM person;");
        assert!(
            graph.iter().any(|finding| finding.code().number() == 1002
                && finding.message().contains("`user` has no field `aeg`")),
            "expected the selection itself to be reported: {:?}",
            graph
                .iter()
                .map(surrealguard_diagnostics::Finding::message)
                .collect::<Vec<_>>()
        );

        let schema = user_with_team_schema();
        let (_, row) = analyze_diagnostics(&schema, "SELECT team.{nope}.nope AS r FROM user;");
        assert!(
            row.iter()
                .any(|finding| finding.code().number() == 1002
                    && finding.message().contains("`nope`")),
            "expected the selection itself to be reported: {:?}",
            row.iter()
                .map(surrealguard_diagnostics::Finding::message)
                .collect::<Vec<_>>()
        );
    }

    // -----------------------------------------------------------------------
    // The EMPTY destructure
    //
    // `.{}` is valid SurrealQL and evaluates to the empty object. It used to be
    // an S0001 parse error, which is fatal to a whole source rather than to one
    // expression.
    // -----------------------------------------------------------------------

    /// `SELECT id.{} FROM user:ada` -> `[{id: {}}]`: the key is still
    /// projected, holding the empty object.
    #[test]
    fn an_empty_row_destructure_still_projects_its_key() {
        let schema = user_with_team_schema();

        let (kind, diagnostics) = analyze_diagnostics(&schema, "SELECT team.{} FROM user;");
        let fields = object_fields(array_element(&kind));

        assert_eq!(fields["team"], object_literal(BTreeMap::new()));
        assert!(
            !codes(&diagnostics).contains(&1),
            "an empty destructure is not a parse error: {:?}",
            codes(&diagnostics)
        );
    }

    /// `SELECT *, author.{} FROM post` narrows the seeded key to `{}` and
    /// leaves its siblings whole (3.0.5 live).
    #[test]
    fn an_empty_destructure_beside_a_wildcard_narrows_the_field_to_nothing() {
        let schema = schema_from(WILDCARD_SIBLING_SCHEMA);

        let fields = row_fields(&schema, "SELECT *, address.{} FROM person;");

        assert_eq!(fields["address"], object_literal(BTreeMap::new()));
        assert_eq!(fields.len(), 4, "the siblings are untouched");
    }

    /// `SELECT ->follows->user.{} AS r FROM user` -> `{r: [{}]}`: one empty
    /// object per traversed record, not one empty projection.
    #[test]
    fn an_empty_graph_destructure_projects_one_empty_object_per_record() {
        let schema = person_friend_user_schema();

        let kind = analyze(&schema, "SELECT ->friend->user.{} AS r FROM person;");
        let fields = object_fields(array_element(&kind));

        assert_eq!(
            fields["r"],
            Kind::Array(Box::new(object_literal(BTreeMap::new())), None)
        );
    }

    /// Nothing was selected, so nothing can be read back out.
    #[test]
    fn a_field_after_an_empty_destructure_is_reported() {
        let schema = user_with_team_schema();

        let (_, diagnostics) = analyze_diagnostics(&schema, "SELECT team.{}.label AS r FROM user;");

        assert!(
            diagnostics
                .iter()
                .any(|finding| finding.code().number() == 1002
                    && finding.message().contains("selected nothing")),
            "expected 1002 naming the empty selection: {:?}",
            diagnostics
                .iter()
                .map(surrealguard_diagnostics::Finding::message)
                .collect::<Vec<_>>()
        );
    }

    /// `SELECT VALUE author.{name} FROM post` is the destructured OBJECT, not
    /// a row keyed by `author` (3.0.5: `[{name: 'Ada'}]`).
    #[test]
    fn a_value_destructure_is_the_object_itself_not_a_row() {
        let schema = user_with_team_schema();

        let kind = analyze(&schema, "SELECT VALUE team.{label} FROM user;");
        let selected = object_fields(array_element(&kind));

        assert_eq!(selected["label"], Kind::String);
        assert_eq!(selected.len(), 1, "no `team` key wrapping it");
    }

    /// And the empty one likewise: `SELECT VALUE id.{} FROM ONLY user:ada`
    /// -> `{}`.
    #[test]
    fn a_value_empty_destructure_is_the_empty_object() {
        let schema = user_with_team_schema();

        let kind = analyze(&schema, "SELECT VALUE team.{} FROM user;");

        assert_eq!(
            kind,
            Kind::Array(Box::new(object_literal(BTreeMap::new())), None)
        );
    }

    #[test]
    fn row_destructure_absent_field_emits_1002() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD profile.email ON person TYPE string;\n\
             DEFINE FIELD profile.city ON person TYPE string;",
        );

        let (kind, diagnostics) =
            analyze_diagnostics(&schema, "SELECT profile.{email, nope} FROM person;");
        // `email` still projects.
        let fields = object_fields(array_element(&kind));
        let profile = object_fields(&fields["profile"]);
        assert_eq!(profile["email"], Kind::String);

        let finding = diagnostics
            .iter()
            .find(|f| f.code().number() == 1002)
            .expect("expected 1002 for the absent row-destructure field");
        assert!(
            finding
                .message()
                .contains("`person` has no field `profile.nope`"),
            "unexpected message: {}",
            finding.message()
        );
    }

    #[test]
    fn record_link_destructure_validates_against_the_linked_table() {
        let schema = user_with_team_schema();

        // `team.{label}` is a record-link destructure: `label` resolves on
        // `team`; a bogus field reports against `team`.
        let (kind, _) = analyze_diagnostics(&schema, "SELECT team.{label} FROM user;");
        let fields = object_fields(array_element(&kind));
        let team = object_fields(&fields["team"]);
        assert_eq!(team["label"], Kind::String);

        let (_, diagnostics) = analyze_diagnostics(&schema, "SELECT team.{nope} FROM user;");
        let finding = diagnostics
            .iter()
            .find(|f| f.code().number() == 1002)
            .expect("expected 1002 for the absent linked destructure field");
        assert!(
            finding.message().contains("`team` has no field `nope`"),
            "unexpected message: {}",
            finding.message()
        );
    }

    /// TI-2: `option`/`array`/`set`-wrapped record links are links too. The
    /// traversal must resolve on the linked table and carry the wrappers back,
    /// so optionality is preserved and field access distributes over a
    /// collection of links.
    #[test]
    fn wrapped_record_links_resolve_and_keep_their_wrappers() {
        let schema = schema_from(
            "DEFINE TABLE user SCHEMAFULL;\n\
             DEFINE FIELD name ON user TYPE string;\n\
             DEFINE TABLE team SCHEMAFULL;\n\
             DEFINE FIELD owner ON team TYPE record<user>;\n\
             DEFINE FIELD lead ON team TYPE option<record<user>>;\n\
             DEFINE FIELD members ON team TYPE array<record<user>>;\n\
             DEFINE FIELD watchers ON team TYPE set<record<user>>;\n\
             DEFINE FIELD opt_members ON team TYPE option<array<record<user>>>;",
        );

        let (kind, diagnostics) = analyze_diagnostics(
            &schema,
            "SELECT owner.name AS o, lead.name AS l, members.name AS m, \
             watchers.name AS w, opt_members.name AS om FROM team;",
        );
        let fields = object_fields(array_element(&kind));

        // Control: a bare link is unchanged.
        assert_eq!(fields["o"], Kind::String);
        // The link can be NONE, so the field access can be too — the
        // optionality must survive the traversal, not be silently stripped.
        assert_eq!(fields["l"], Kind::Either(vec![Kind::None, Kind::String]));
        // Field access distributes over a collection of links.
        assert_eq!(fields["m"], Kind::Array(Box::new(Kind::String), None));
        assert_eq!(fields["w"], Kind::Set(Box::new(Kind::String), None));
        // Both wrappers, outermost first.
        assert_eq!(
            fields["om"],
            Kind::Either(vec![Kind::None, Kind::Array(Box::new(Kind::String), None)])
        );
        assert!(
            !codes(&diagnostics).contains(&1002),
            "valid paths through wrapped links must not emit 1002: {:?}",
            codes(&diagnostics)
        );
    }

    #[test]
    fn a_path_into_a_refined_parent_resolves_through_its_declared_kind() {
        // A descendant DEFINE FIELD refines the parent's kind in place rather
        // than becoming a sibling field, so the prefix scan never sees it.
        // Projecting into the parent must still resolve — and must carry the
        // parent's optionality, since an absent `cfg` makes `cfg.theme` absent.
        let schema = schema_from(
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD cfg ON t TYPE option<object>;\n\
             DEFINE FIELD cfg.theme ON t TYPE string;",
        );

        let (kind, diagnostics) = analyze_diagnostics(&schema, "SELECT cfg.theme AS th FROM t;");
        let fields = object_fields(array_element(&kind));
        assert_eq!(
            fields["th"],
            Kind::Either(vec![Kind::None, Kind::String]),
            "a path into a refined parent must resolve, keeping the parent's optionality"
        );
        assert!(
            !codes(&diagnostics).contains(&1002),
            "a declared subfield is not a missing field: {:?}",
            codes(&diagnostics)
        );

        // Negative: an undeclared subfield stays unresolved rather than being
        // invented from the refined object.
        let (kind, _) = analyze_diagnostics(&schema, "SELECT cfg.nope AS n FROM t;");
        let fields = object_fields(array_element(&kind));
        assert_eq!(fields["n"], Kind::Any);
    }

    #[test]
    fn a_destructure_over_a_wrapped_link_hoists_the_wrapper_to_the_object() {
        // `members.{name}` projects one object PER LINKED RECORD, so SurrealDB
        // returns `array<{ name: string }>` — not `{ name: array<string> }`.
        // The wrapper belongs to the object, not to each selected field.
        let schema = schema_from(
            "DEFINE TABLE user SCHEMAFULL;\n\
             DEFINE FIELD name ON user TYPE string;\n\
             DEFINE TABLE team SCHEMAFULL;\n\
             DEFINE FIELD owner ON team TYPE record<user>;\n\
             DEFINE FIELD lead ON team TYPE option<record<user>>;\n\
             DEFINE FIELD members ON team TYPE array<record<user>>;",
        );

        let (kind, _) = analyze_diagnostics(
            &schema,
            "SELECT owner.{name} AS o, lead.{name} AS l, members.{name} AS m FROM team;",
        );
        let fields = object_fields(array_element(&kind));

        let name_object = object_literal(
            [("name".to_string(), Kind::String)]
                .into_iter()
                .collect::<BTreeMap<_, _>>(),
        );
        // Control: a bare link destructures to a plain object.
        assert_eq!(fields["o"], name_object);
        // The whole object is optional — the link may be NONE.
        assert_eq!(
            fields["l"],
            Kind::Either(vec![Kind::None, name_object.clone()])
        );
        // The collection wraps the object, not the field.
        assert_eq!(
            fields["m"],
            Kind::Array(Box::new(name_object), None),
            "a destructure over `array<record<T>>` must be `array<object>`"
        );
    }

    /// TI-2: once a wrapped link resolves, the unknown-field check that
    /// `field_is_opaque_boundary` used to suppress must come back — the
    /// remainder is checked on the *linked* table, exactly as for a bare link.
    #[test]
    fn an_absent_field_past_a_wrapped_link_emits_1002() {
        let schema = schema_from(
            "DEFINE TABLE user SCHEMAFULL;\n\
             DEFINE FIELD name ON user TYPE string;\n\
             DEFINE TABLE team SCHEMAFULL;\n\
             DEFINE FIELD lead ON team TYPE option<record<user>>;\n\
             DEFINE FIELD members ON team TYPE array<record<user>>;",
        );

        for query in [
            "SELECT lead.bogus FROM team;",
            "SELECT members.bogus FROM team;",
            "SELECT lead.{bogus} FROM team;",
        ] {
            let (_, diagnostics) = analyze_diagnostics(&schema, query);
            let finding = diagnostics
                .iter()
                .find(|f| f.code().number() == 1002)
                .unwrap_or_else(|| panic!("expected 1002 for `{query}`"));
            assert!(
                finding.message().contains("`user` has no field `bogus`"),
                "unexpected message: {}",
                finding.message()
            );
        }
    }

    /// TI-2: expression positions check the path against the row table without
    /// crossing links, so an unresolvable wrapped link used to read as an
    /// absent field — a false 1002 on a perfectly valid `WHERE`.
    #[test]
    fn a_wrapped_link_in_a_where_clause_is_not_a_missing_field() {
        let schema = schema_from(
            "DEFINE TABLE user SCHEMAFULL;\n\
             DEFINE FIELD name ON user TYPE string;\n\
             DEFINE TABLE team SCHEMAFULL;\n\
             DEFINE FIELD owner ON team TYPE record<user>;\n\
             DEFINE FIELD lead ON team TYPE option<record<user>>;\n\
             DEFINE FIELD members ON team TYPE array<record<user>>;",
        );

        let (_, diagnostics) = analyze_diagnostics(
            &schema,
            "SELECT id FROM team WHERE lead.name = 'x' AND owner.name = 'y' \
             AND members.name CONTAINS 'z';",
        );
        assert!(
            !codes(&diagnostics).contains(&1002),
            "a valid path through a wrapped link must not read as an absent field: {:?}",
            codes(&diagnostics)
        );
    }

    /// The other half of the same contract: a condition resolves a path the way
    /// a projection of it does, so a field absent on the *linked* table is
    /// reported there. `WHERE owner.ghost = 1` was silent while `SELECT
    /// owner.ghost` reported, because the condition walk stopped at the link.
    /// A mutation's filter is the same walk; `tests/corpus/invalid/` pins the
    /// `UPDATE`/`DELETE` spellings, which this SELECT-only harness cannot lower.
    #[test]
    fn an_absent_field_past_a_link_in_a_where_clause_emits_1002() {
        let schema = schema_from(
            "DEFINE TABLE user SCHEMAFULL;\n\
             DEFINE FIELD name ON user TYPE string;\n\
             DEFINE FIELD boss ON user TYPE option<record<user>>;\n\
             DEFINE TABLE team SCHEMAFULL;\n\
             DEFINE FIELD owner ON team TYPE record<user>;",
        );

        for query in [
            "SELECT id FROM team WHERE owner.ghost = 1;",
            "SELECT id FROM team WHERE owner.boss.ghost = 1;",
            "SELECT id FROM team WHERE id != NONE AND (owner.ghost = 1 OR id = NONE);",
            "SELECT id FROM team WHERE string::len(owner.ghost) > 0;",
        ] {
            let (_, diagnostics) = analyze_diagnostics(&schema, query);
            let finding = diagnostics
                .iter()
                .find(|finding| finding.code().number() == 1002)
                .unwrap_or_else(|| panic!("expected 1002 for `{query}`"));
            assert!(
                finding.message().contains("`user` has no field `ghost`"),
                "unexpected message for `{query}`: {}",
                finding.message()
            );
        }
    }

    /// The false positive the same routing removes. A `TYPE object` field is
    /// open — any key may be there — which is why the projection has never
    /// reported a subpath of one. The condition claimed the row had no field
    /// `settings.anything`, on a read that is perfectly legal.
    #[test]
    fn a_subpath_of_an_open_object_in_a_where_clause_is_not_a_missing_field() {
        let schema = schema_from(
            "DEFINE TABLE team SCHEMAFULL;\n\
             DEFINE FIELD name ON team TYPE string;\n\
             DEFINE FIELD settings ON team FLEXIBLE TYPE object;",
        );

        let (_, diagnostics) = analyze_diagnostics(
            &schema,
            "SELECT name FROM team WHERE settings.anything = 1;",
        );
        assert!(
            !codes(&diagnostics).contains(&1002),
            "a subpath of an open object must not read as an absent field: {:?}",
            codes(&diagnostics)
        );
    }

    /// TI-2, negative: a union with a record arm *and* an unrelated arm has no
    /// single payload to traverse. Stay conservative — no invented type, and
    /// no 1002 on a remainder we cannot prove absent.
    #[test]
    fn a_union_that_is_only_partly_a_link_stays_conservative() {
        let schema = schema_from(
            "DEFINE TABLE user SCHEMAFULL;\n\
             DEFINE FIELD name ON user TYPE string;\n\
             DEFINE TABLE team SCHEMAFULL;\n\
             DEFINE FIELD ambiguous ON team TYPE record<user> | int;",
        );
        // Sanity: the field really is a two-armed union, not a plain link.
        assert!(
            crate::kinds::record_link_shape(
                schema.tables["team"].fields["ambiguous"]
                    .kind
                    .as_ref()
                    .expect("declared kind")
            )
            .is_none(),
            "the probe field must not peel to a record link"
        );

        let (_, diagnostics) = analyze_diagnostics(&schema, "SELECT ambiguous.bogus FROM team;");
        assert!(
            !codes(&diagnostics).contains(&1002),
            "an unprovable union must not be traversed: {:?}",
            codes(&diagnostics)
        );
    }

    #[test]
    fn destructure_against_schemaless_target_emits_no_1002() {
        // `user` here has no declared fields (schemaless): field-level checks
        // are skipped by design, so a destructure emits no false positive.
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD name ON person TYPE string;\n\
             DEFINE TABLE user SCHEMAFULL;\n\
             DEFINE TABLE friend TYPE RELATION IN person OUT user;",
        );

        let (_, diagnostics) =
            analyze_diagnostics(&schema, "SELECT ->friend->user.{whatever} FROM person;");
        assert!(
            !codes(&diagnostics).contains(&1002),
            "a schemaless destructure target must not emit 1002: {:?}",
            codes(&diagnostics)
        );
    }

    #[test]
    fn traversal_through_an_opaque_field_emits_no_1002() {
        // `account.person` is COMPUTED with no explicit TYPE, so its kind is
        // unknown (`None`/`Any`). We cannot cross it to a concrete table, so we
        // cannot prove `first_name`/`last_name` absent — suppress (no FP). This
        // is the workshop-oracle repro (schema/organization/employee_of.surql).
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD first_name ON person TYPE string;\n\
             DEFINE FIELD last_name ON person TYPE string;\n\
             DEFINE TABLE account SCHEMAFULL;\n\
             DEFINE FIELD settings ON account TYPE string;\n\
             DEFINE FIELD person ON account COMPUTED <~person[0];",
        );

        // Sanity: the field exists but has no resolvable kind.
        let account = &schema.tables["account"];
        assert!(account.fields.contains_key("person"));
        assert!(
            !matches!(account.fields["person"].kind, Some(Kind::Record(_))),
            "the COMPUTED field must not resolve to a concrete record link"
        );

        // Row-destructure through the opaque field: no 1002.
        let (_, diagnostics) = analyze_diagnostics(
            &schema,
            "SELECT id, person.{first_name, last_name}, settings FROM ONLY account WHERE id = account:x;",
        );
        assert!(
            !codes(&diagnostics).contains(&1002),
            "destructure through an opaque field must not emit 1002: {:?}",
            codes(&diagnostics)
        );

        // Plain traversal through the opaque field: no 1002 either.
        let (_, diagnostics) =
            analyze_diagnostics(&schema, "SELECT person.first_name FROM account;");
        assert!(
            !codes(&diagnostics).contains(&1002),
            "plain traversal through an opaque field must not emit 1002: {:?}",
            codes(&diagnostics)
        );
    }

    // -----------------------------------------------------------------------
    // WHERE-narrowing of the projected row type (design §3.1)
    // -----------------------------------------------------------------------

    fn narrowing_schema() -> SchemaIndex {
        schema_from(
            "DEFINE TABLE user SCHEMAFULL;\n\
             DEFINE TABLE admin SCHEMAFULL;\n\
             DEFINE FIELD name ON user TYPE string;\n\
             DEFINE FIELD email ON user TYPE option<string>;\n\
             DEFINE FIELD age ON user TYPE option<int>;\n\
             DEFINE FIELD status ON user TYPE string;\n\
             DEFINE FIELD role ON user TYPE string;\n\
             DEFINE FIELD country ON user TYPE string;\n\
             DEFINE FIELD owner ON user TYPE record<user | admin>;",
        )
    }

    fn option_string() -> Kind {
        Kind::Either(vec![Kind::None, Kind::String])
    }

    fn option_int() -> Kind {
        Kind::Either(vec![Kind::None, Kind::Int])
    }

    #[test]
    fn where_not_none_strips_none_from_the_projected_field() {
        let schema = narrowing_schema();
        // Baseline: without the guard, `email` keeps its option.
        let baseline = analyze(&schema, "SELECT email FROM user;");
        assert_eq!(
            object_fields(array_element(&baseline))["email"],
            option_string()
        );

        let kind = analyze(&schema, "SELECT email FROM user WHERE email != NONE;");
        assert_eq!(object_fields(array_element(&kind))["email"], Kind::String);
    }

    #[test]
    fn where_literal_eq_pins_the_field_to_the_literal() {
        let schema = narrowing_schema();
        let kind = analyze(&schema, "SELECT status FROM user WHERE status = 'active';");
        assert_eq!(
            object_fields(array_element(&kind))["status"],
            Kind::Literal(KindLiteral::String("active".into()))
        );
    }

    #[test]
    fn where_greater_than_strips_the_option() {
        let schema = narrowing_schema();
        let kind = analyze(&schema, "SELECT age FROM user WHERE age > 18;");
        assert_eq!(object_fields(array_element(&kind))["age"], Kind::Int);
    }

    #[test]
    fn where_less_than_narrows_nothing() {
        // Soundness regression guard: `NONE < 65` is TRUE, so NONE rows
        // survive; `age` must keep its option.
        let schema = narrowing_schema();
        let kind = analyze(&schema, "SELECT age FROM user WHERE age < 65;");
        assert_eq!(object_fields(array_element(&kind))["age"], option_int());
    }

    #[test]
    fn where_type_table_narrows_the_record_union() {
        let schema = narrowing_schema();
        let baseline = analyze(&schema, "SELECT owner FROM user;");
        assert_eq!(
            object_fields(array_element(&baseline))["owner"],
            Kind::Record(vec!["user".into(), "admin".into()])
        );

        let kind = analyze(
            &schema,
            "SELECT owner FROM user WHERE type::table(owner) = 'user';",
        );
        assert_eq!(
            object_fields(array_element(&kind))["owner"],
            Kind::Record(vec!["user".into()])
        );
    }

    #[test]
    fn where_and_unions_both_effects() {
        let schema = narrowing_schema();
        let kind = analyze(
            &schema,
            "SELECT email, owner FROM user WHERE email != NONE AND type::table(owner) = 'user';",
        );
        let fields = object_fields(array_element(&kind));
        assert_eq!(fields["email"], Kind::String);
        assert_eq!(fields["owner"], Kind::Record(vec!["user".into()]));
    }

    #[test]
    fn where_or_narrows_only_what_every_disjunct_proves() {
        let schema = narrowing_schema();
        let query = "SELECT role FROM user WHERE role = 'admin' OR role = 'mod';";
        let role = |kind: Kind| object_fields(array_element(&kind))["role"].clone();

        // The disjuncts join, because BOTH pin the same place: every surviving
        // row has one of the two values. A disjunct about a different field
        // would still prove nothing — that is
        // `a_disjunction_refines_only_what_every_arm_refines` in `facts::refine`.
        assert_eq!(
            role(analyze(&schema, query)),
            Kind::either(vec![
                Kind::Literal(surrealdb_types::KindLiteral::String("admin".into())),
                Kind::Literal(surrealdb_types::KindLiteral::String("mod".into())),
            ])
        );
    }

    #[test]
    fn group_by_disables_narrowing() {
        let schema = narrowing_schema();
        let kind = analyze(
            &schema,
            "SELECT email FROM user WHERE email != NONE GROUP BY country;",
        );
        // `email` is neither the group key nor an aggregate, so each group
        // collects it (4029) — and the collected element is the *declared*
        // `option<string>`, not the WHERE-narrowed `string`.
        assert_eq!(
            object_fields(array_element(&kind))["email"],
            Kind::Array(Box::new(option_string()), None)
        );
    }

    #[test]
    fn a_value_projection_narrows_the_row_itself() {
        let schema = narrowing_schema();
        // A `Place` is not an output key: `VALUE email` says the row IS that
        // place, so the refinement applies to the row kind. Keying refinements
        // by projected field path instead — as the recognizer this replaced
        // did — left nowhere to put one, and disabled the pass outright.
        assert_eq!(
            analyze(&schema, "SELECT VALUE email FROM user WHERE email != NONE;"),
            Kind::Array(Box::new(Kind::String), None)
        );
    }

    #[test]
    fn an_aliased_projection_narrows_under_its_alias() {
        let schema = narrowing_schema();
        let query = "SELECT email AS e FROM user WHERE email != NONE;";
        let aliased = |kind: Kind| object_fields(array_element(&kind))["e"].clone();

        // The projection says where the place landed, so the fact reaches it
        // under the alias. Matching by projected key alone — the refinement
        // keyed `["email"]` against a row whose only key is `e` — reached
        // nothing.
        assert_eq!(aliased(analyze(&schema, query)), Kind::String);
    }

    #[test]
    fn a_field_projected_twice_narrows_under_both_names() {
        let schema = narrowing_schema();
        // Both keys hold the same value of the same row, so one fact tightens
        // both — the alias route does not replace the plain one.
        let kind = analyze(
            &schema,
            "SELECT email, email AS e FROM user WHERE email != NONE;",
        );
        let fields = object_fields(array_element(&kind));
        assert_eq!(fields["email"], Kind::String);
        assert_eq!(fields["e"], Kind::String);
    }

    #[test]
    fn a_computed_alias_is_not_the_place_the_guard_names() {
        // `string::len(email) AS e` is a *function of* the guarded place, not
        // the place — narrowing it would claim something about a value the
        // guard says nothing about.
        let schema = narrowing_schema();
        let kind = analyze(
            &schema,
            "SELECT string::len(email) AS e FROM user WHERE email != NONE;",
        );
        assert_ne!(object_fields(array_element(&kind))["e"], Kind::String);
    }

    #[test]
    fn a_guard_on_a_non_projected_field_is_a_noop() {
        let schema = narrowing_schema();
        let kind = analyze(&schema, "SELECT name FROM user WHERE email != NONE;");
        let fields = object_fields(array_element(&kind));
        assert_eq!(fields["name"], Kind::String);
        assert!(!fields.contains_key("email"));
    }

    #[test]
    fn a_sibling_field_untouched_by_any_guard_keeps_its_schema_kind() {
        let schema = narrowing_schema();
        let kind = analyze(&schema, "SELECT email, age FROM user WHERE email != NONE;");
        let fields = object_fields(array_element(&kind));
        assert_eq!(fields["email"], Kind::String);
        // `age` is untouched by the guard.
        assert_eq!(fields["age"], option_int());
    }

    #[test]
    fn subquery_source_disables_narrowing() {
        let schema = narrowing_schema();
        let kind = analyze(
            &schema,
            "SELECT * FROM (SELECT email FROM user) WHERE email != NONE;",
        );
        assert_eq!(
            object_fields(array_element(&kind))["email"],
            option_string()
        );
    }

    #[test]
    fn opacity_suppression_still_reports_a_genuinely_typed_absent_link_field() {
        // Guard against over-suppression: a properly typed record link to an
        // absent field must still emit 1002 (the opacity carve-out is narrow).
        let schema = user_with_team_schema();

        let (_, diagnostics) = analyze_diagnostics(&schema, "SELECT team.{nope} FROM user;");
        assert!(
            codes(&diagnostics).contains(&1002),
            "a typed link to an absent field must still emit 1002: {:?}",
            codes(&diagnostics)
        );
    }

    // -- GROUP BY / ORDER BY over a projection alias (DX-1) ------------------

    fn alias_group_schema() -> SchemaIndex {
        schema_from(
            "DEFINE TABLE product SCHEMAFULL;\n\
             DEFINE FIELD price ON product TYPE number;\n\
             DEFINE FIELD team ON product TYPE string;",
        )
    }

    #[test]
    fn group_by_a_projection_alias_is_not_an_unknown_field() {
        let schema = alias_group_schema();

        for query in [
            "SELECT price AS n FROM product GROUP BY n;",
            "SELECT team AS t, count() FROM product GROUP BY t;",
        ] {
            let diagnostics = diagnostics_for(&schema, query);
            assert!(
                !codes(&diagnostics).contains(&1002),
                "`{query}` must not report an unknown field: {:?}",
                codes(&diagnostics)
            );
        }
    }

    #[test]
    fn order_by_a_projection_alias_is_not_an_unknown_field() {
        let schema = alias_group_schema();

        for query in [
            "SELECT price AS n FROM product ORDER BY n;",
            "SELECT price AS n, count() FROM product GROUP BY n ORDER BY n;",
        ] {
            let diagnostics = diagnostics_for(&schema, query);
            assert!(
                !codes(&diagnostics).contains(&1002),
                "`{query}` must not report an unknown field: {:?}",
                codes(&diagnostics)
            );
        }
    }

    #[test]
    fn group_by_an_alias_nested_path_is_covered_by_the_alias() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD address ON person TYPE object;\n\
             DEFINE FIELD address.city ON person TYPE string;",
        );

        let diagnostics =
            diagnostics_for(&schema, "SELECT address AS a FROM person GROUP BY a.city;");
        assert!(
            !codes(&diagnostics).contains(&1002),
            "a path under a projected alias must not report an unknown field: {:?}",
            codes(&diagnostics)
        );
    }

    // -- aggregates over computed columns (DX-2) -----------------------------

    fn aggregate_schema() -> SchemaIndex {
        schema_from(
            "DEFINE TABLE post SCHEMAFULL;\n\
             DEFINE FIELD price ON post TYPE number;\n\
             DEFINE FIELD qty ON post TYPE int;\n\
             DEFINE FIELD tags ON post TYPE array<string>;",
        )
    }

    #[test]
    fn aggregate_over_a_computed_column_is_not_a_scalar_argument() {
        // The aggregate is handed the collected column whatever expression
        // produced it, so none of these violate the `array` contract.
        let schema = aggregate_schema();

        // The expected total kind follows the collected column: an `int`
        // column totals to an `int`, a `float`/`number` one to a `number`.
        for (query, expected) in [
            (
                "SELECT math::sum(price * qty) AS a FROM post GROUP ALL;",
                Kind::Number,
            ),
            (
                "SELECT math::mean(qty * 1) AS a FROM post GROUP ALL;",
                Kind::Number,
            ),
            (
                "SELECT math::sum(<float> qty) AS a FROM post GROUP ALL;",
                Kind::Number,
            ),
        ] {
            let (kind, diagnostics) = analyze_diagnostics(&schema, query);
            assert!(
                !codes(&diagnostics).contains(&5002),
                "`{query}` must not report an argument violation: {:?}",
                codes(&diagnostics)
            );
            assert_eq!(
                object_fields(array_element(&kind))["a"],
                expected,
                "for `{query}`"
            );
        }
    }

    #[test]
    fn aggregate_nested_in_a_computation_is_still_promoted() {
        let schema = aggregate_schema();

        for query in [
            "SELECT math::sum(qty) * 2 AS a FROM post GROUP ALL;",
            "SELECT math::sum(price) + math::sum(qty) AS a FROM post GROUP ALL;",
            "SELECT math::sum(price) / math::sum(qty) + qty AS a FROM post GROUP ALL;",
        ] {
            let (kind, diagnostics) = analyze_diagnostics(&schema, query);
            assert!(
                !codes(&diagnostics).contains(&5002),
                "`{query}` must not report an argument violation: {:?}",
                codes(&diagnostics)
            );
            // The computation around the aggregate types normally (the exact
            // numeric kind is the arithmetic engine's business).
            let computed = &object_fields(array_element(&kind))["a"];
            assert!(
                crate::analyzer::expression::infer::is_numeric(computed),
                "`{query}` should compute a numeric column, got {computed:?}"
            );
        }
    }

    #[test]
    fn aggregate_under_a_prefix_operator_is_still_promoted() {
        let schema = aggregate_schema();

        let (kind, diagnostics) =
            analyze_diagnostics(&schema, "SELECT !math::sum(qty) AS a FROM post GROUP ALL;");
        assert!(
            !codes(&diagnostics).contains(&5002),
            "a negated aggregate must not report an argument violation: {:?}",
            codes(&diagnostics)
        );
        assert_eq!(object_fields(array_element(&kind))["a"], Kind::Bool);
    }

    #[test]
    fn a_collection_column_keeps_its_element_wise_aggregate_reading() {
        // The promotion must not double-wrap a column that is already an
        // array: `math::max(tags)` takes the column itself.
        let schema = aggregate_schema();

        let (_, diagnostics) =
            analyze_diagnostics(&schema, "SELECT math::max(tags) AS m FROM post GROUP ALL;");
        assert!(
            !codes(&diagnostics).contains(&5002),
            "an array column must not report an argument violation: {:?}",
            codes(&diagnostics)
        );
    }

    #[test]
    fn misuse_inside_and_around_an_aggregate_still_reports_once() {
        // Guard against over-suppression: promoting the aggregate must not
        // swallow the contract violations of the expressions it is built
        // from, nor report them twice.
        let schema = aggregate_schema();

        for query in [
            // scalar handed to a non-aggregate array function, beside an aggregate
            "SELECT math::sum(price) + array::len(qty) AS a FROM post GROUP ALL;",
            // ... and inside the aggregate's own argument
            "SELECT math::sum(array::len(price)) AS a FROM post GROUP ALL;",
        ] {
            let (_, diagnostics) = analyze_diagnostics(&schema, query);
            let violations = codes(&diagnostics)
                .into_iter()
                .filter(|code| *code == 5002)
                .count();
            assert_eq!(
                violations,
                1,
                "`{query}` must report its argument violation exactly once: {:?}",
                codes(&diagnostics)
            );
        }
    }

    #[test]
    fn aggregate_arity_is_still_checked() {
        let schema = aggregate_schema();

        let (_, diagnostics) = analyze_diagnostics(
            &schema,
            "SELECT math::sum(price, qty) AS a FROM post GROUP ALL;",
        );
        assert!(
            codes(&diagnostics).contains(&5002),
            "a two-argument `math::sum` must still be reported: {:?}",
            codes(&diagnostics)
        );
    }

    #[test]
    fn group_by_and_order_by_a_genuinely_unknown_field_still_error() {
        // Guard against over-suppression: the alias carve-out must not
        // disable the check for a key that names nothing.
        let schema = alias_group_schema();

        for query in [
            "SELECT price AS n FROM product GROUP BY nope;",
            "SELECT price AS n FROM product ORDER BY nope;",
            "SELECT * FROM product GROUP BY nope;",
            "SELECT * FROM product ORDER BY nope;",
        ] {
            let diagnostics = diagnostics_for(&schema, query);
            assert!(
                codes(&diagnostics).contains(&1002),
                "`{query}` must still report an unknown field: {:?}",
                codes(&diagnostics)
            );
        }
    }
}
