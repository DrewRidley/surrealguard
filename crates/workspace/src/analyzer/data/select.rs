//! `SELECT` statement analysis: response-type inference over the typed AST.
//!
//! The inferred response type is a plain upstream `Kind`: closed objects
//! are `Kind::Literal(KindLiteral::Object(...))`, undeterminable positions
//! are `Kind::Any` poison values. Graph traversals, destructure selections,
//! and modifier clauses are consumed as structured `ast::*` values. The
//! only use of source text is *naming*: an unaliased computed projection is
//! keyed by its own source text (`SELECT age >= 18 FROM ...` produces the
//! field `"age >= 18"`).

use std::collections::BTreeMap;

use surrealdb_types::{Kind, KindLiteral};
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::expression::infer::{infer_expression_fact, plain_field_segments};
use crate::schema::{SchemaIndex, TableDef};

pub(crate) fn analyze_select(ctx: &mut AnalysisContext<'_>, stmt: &ast::SelectStmt) -> Kind {
    select_response_kind(stmt, ctx)
}

/// Pure core: infers the response type of a lowered `SELECT`.
pub(crate) fn select_response_kind(stmt: &ast::SelectStmt, ctx: &mut AnalysisContext<'_>) -> Kind {
    check_select_statement_shape(stmt, ctx);
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

    // Row-context clauses reference fields by name; each position has its
    // own code so hosts can configure them independently.
    for idiom in &stmt.omit {
        if let Some(segments) = plain_field_segments(&idiom.node) {
            crate::analyzer::data::check_field_path(ctx, table, &segments, idiom.span, 1002);
        }
    }
    check_fetch_clauses(stmt, table, ctx);
    check_split_clauses(stmt, table, ctx);
    if let Some(group) = &stmt.group {
        for idiom in &group.keys {
            if let Some(segments) = plain_field_segments(&idiom.node) {
                crate::analyzer::data::check_field_path(ctx, table, &segments, idiom.span, 1002);
            }
        }
    }
    check_order_clause(stmt, table, ctx);

    let row_kind = if stmt
        .projections
        .iter()
        .any(|projection| matches!(projection, ast::Projection::Wildcard(_)))
    {
        object_kind_for_all_fields(table)
    } else if let Some(value_kind) = value_projection_kind(stmt, &table_name, table, ctx) {
        value_kind
    } else {
        projected_object_kind(stmt, &table_name, table, ctx)
    };
    // WHERE-narrowing (design §3.1): tighten each projected field the WHERE
    // clause provably constrains. Runs before OMIT/FETCH/SPLIT, which operate
    // on the same object-literal shape.
    let row_kind = apply_where_narrowing(row_kind, stmt);
    let row_kind = apply_omit(row_kind, &stmt.omit);
    let row_kind = apply_fetch(row_kind, &stmt.fetch, ctx.schema());
    let row_kind = apply_split(row_kind, &stmt.split);

    if stmt.only {
        // NOTE: `FROM ONLY` is really `option<row>` (NONE when nothing matches).
        // Modeling that is deferred: it cascades into AND-guard narrowing that
        // must reach function-call arguments (`IF $x != NONE AND f($x)`), which
        // is follow-up work. Kept as a bare row for now to hold the oracle clean.
        row_kind
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
                row_kind
            } else {
                Kind::Array(Box::new(row_kind), literal_limit(stmt))
            })
        }
        // A parameter source is constrained to the known table names.
        ast::Expr::Param(param) => {
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
            .with_help(format!("add `DEFINE FIELD` declarations to `{table_name}` for full analysis")),
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
        if crate::analyzer::flow::if_else::definitely_not_bool(&kind) {
            let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), cond.span);
            ctx.emit(
                surrealguard_diagnostics::catalog::finding(
                    span,
                    2005,
                    format!("this WHERE condition is a `{}`, not a `bool`", crate::render_kind(&kind)),
                )
                .with_help("a WHERE filter keeps rows where the condition is true; it must be a bool"),
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
                                        crate::render_kind(&kind)
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
            crate::analyzer::data::check_field_path(ctx, table, &segments, idiom.span, 1002);
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
            crate::analyzer::data::check_field_path(ctx, table, &segments, idiom.span, 1002);
            // SPLIT fans rows out over a collection field. Resolve across
            // record links so a linked collection field types precisely.
            if let Some(kind) = resolve_field_path(ctx.schema(), table, &segments) {
                let base = crate::kinds::literal_base_kind(&kind).unwrap_or_else(|| kind.clone());
                if !matches!(base, Kind::Array(_, _) | Kind::Set(_, _) | Kind::Any) {
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
        crate::analyzer::data::check_field_path(ctx, table, &segments, key.expr.span, 1002);
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

/// Clause-value invariants: LIMIT/START must be integers (2018) and
/// non-negative when constant (2024); TIMEOUT takes a duration (2019).
fn check_clause_values(stmt: &ast::SelectStmt, ctx: &mut AnalysisContext<'_>) {
    for (clause, name) in [(&stmt.limit, "LIMIT"), (&stmt.start, "START")] {
        let Some(expr) = clause else {
            continue;
        };
        if let ast::Expr::Param(param) = &expr.node {
            if ctx.env().let_fact(param).is_none() {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), expr.span);
                ctx.constrain_param(
                    param,
                    span,
                    Kind::Int,
                    Some(crate::analysis::ValueDomain::Range {
                        min: Some(0),
                        max: None,
                    }),
                );
                continue;
            }
        }
        let fact = infer_expression_fact(expr, ctx);
        if let Some(kind) = &fact.kind {
            let base = crate::kinds::literal_base_kind(kind).unwrap_or_else(|| kind.clone());
            if !matches!(base, Kind::Int | Kind::Number | Kind::Any) {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), expr.span);
                ctx.emit(surrealguard_diagnostics::catalog::finding(
                    span,
                    2018,
                    format!("{name} needs an integer, but this is a `{}`", crate::render_kind(kind)),
                ));
                continue;
            }
        }
        if let Some(surrealdb_types::Value::Number(surrealdb_types::Number::Int(value))) =
            &fact.value
        {
            if *value < 0 {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), expr.span);
                ctx.emit(surrealguard_diagnostics::catalog::finding(
                    span,
                    2018,
                    format!("{name} can't be negative"),
                ));
            }
        }
    }
    if let Some(expr) = &stmt.timeout {
        let kind = infer_expression_fact(expr, ctx).kind;
        if let Some(kind) = kind {
            if !matches!(kind, Kind::Duration | Kind::Any) {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), expr.span);
                ctx.emit(surrealguard_diagnostics::catalog::finding(
                    span,
                    2019,
                    format!("TIMEOUT needs a duration, but this is a `{}`", crate::render_kind(&kind)),
                ));
            }
        }
    }
}

/// Statement-shape invariants that don't depend on the schema: ONLY
/// without a single-row guarantee (4003 — a deterministic
/// `SingleOnlyOutput` runtime error) and duplicate projection keys (4011).
/// (VALUE's single-projection rule needs no finding: both SurrealDB's
/// parser and ours reject the syntax.)
fn check_select_statement_shape(stmt: &ast::SelectStmt, ctx: &mut AnalysisContext<'_>) {
    check_clause_values(stmt, ctx);
    check_count_without_group(stmt, ctx);

    let has_wildcard = stmt
        .projections
        .iter()
        .any(|projection| matches!(projection, ast::Projection::Wildcard(_)));

    // `SELECT *, age` — the explicit field is already inside `*`.
    if has_wildcard {
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

/// A bare zero-argument `count()` in a projection is an aggregate only under
/// a GROUP clause. Without one, SurrealDB evaluates it *per row*, so every
/// row's `count` is the constant `1` — never the row total the author
/// intended. A guard built on the result (`IF $rows = 0 { THROW ... }`) then
/// silently never fires (4023). `GROUP ALL` / `GROUP BY` make it a real
/// aggregate and clear the finding.
fn check_count_without_group(stmt: &ast::SelectStmt, ctx: &mut AnalysisContext<'_>) {
    if stmt.group.is_some() {
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

/// The zero-argument row-counting form of `count`. Lowering does not fold
/// `count` into `count::count`, so both spellings are accepted.
fn is_bare_count(call: &ast::Call) -> bool {
    call.args.is_empty() && matches!(call.path.node.as_str(), "count" | "count::count")
}

/// Whether a kind can transitively hold record links (making FETCH
/// meaningful): records themselves, collections/options/unions of them.
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

/// Walks graph steps in edge/target pairs, checking each edge's relation
/// endpoints. Returns the final target table.
fn resolve_graph_chain(
    source_table: &str,
    parts: &[ast::Spanned<ast::IdiomPart>],
    schema: &SchemaIndex,
) -> Option<String> {
    if parts.is_empty() || !parts.len().is_multiple_of(2) {
        return None;
    }

    let mut current = source_table.to_string();
    for pair in parts.chunks_exact(2) {
        let (dir, edge) = single_graph_target(&pair[0].node)?;
        let (_, target) = single_graph_target(&pair[1].node)?;
        current = relation_step_target(&current, dir, edge, target, schema)?;
    }
    Some(current)
}

/// A graph part's direction and single target table. Multi-target steps
/// (`->(a, b)`) do not resolve to one table and stay unresolved.
fn single_graph_target(part: &ast::IdiomPart) -> Option<(ast::GraphDir, &str)> {
    let ast::IdiomPart::Graph { dir, step } = part else {
        return None;
    };
    match step.targets.as_slice() {
        [only] => Some((dir.node, only.node.as_str())),
        _ => None,
    }
}

/// Does `edge`'s relation connect `source_table` to `target` in
/// direction `dir`?
fn relation_step_target(
    source_table: &str,
    dir: ast::GraphDir,
    edge: &str,
    target: &str,
    schema: &SchemaIndex,
) -> Option<String> {
    let relation = schema.tables.get(edge)?.relation.as_ref()?;
    let source_in = relation.in_tables.iter().any(|t| t == source_table);
    let source_out = relation.out_tables.iter().any(|t| t == source_table);
    let target_in = relation.in_tables.iter().any(|t| t == target);
    let target_out = relation.out_tables.iter().any(|t| t == target);

    let connected = match dir {
        ast::GraphDir::Out => source_in && target_out,
        ast::GraphDir::In => source_out && target_in,
        ast::GraphDir::Both => (source_in && target_out) || (source_out && target_in),
    };
    connected.then(|| target.to_string())
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
pub(crate) fn reference_back_traversal_kind(
    self_table: &str,
    idiom: &ast::Idiom,
    schema: &SchemaIndex,
) -> Option<Kind> {
    let [only] = idiom.parts.as_slice() else {
        return None;
    };
    let ast::IdiomPart::Graph { dir, step } = &only.node else {
        return None;
    };
    // A back-reference is an incoming reference step (`<~`) at a single named
    // target, with no inline selection.
    if dir.node != ast::GraphDir::In
        || !step.reference
        || step.where_clause.is_some()
    {
        return None;
    }
    let [target] = step.targets.as_slice() else {
        return None;
    };
    let target_name = target.node.as_str();
    let target_table = schema.tables.get(target_name)?;
    let points_back = target_table
        .fields
        .values()
        .any(|field| field.reference && kind_targets_table(field.kind.as_ref(), self_table));
    points_back
        .then(|| Kind::Array(Box::new(Kind::Record(vec![target_name.into()])), None))
}

/// Whether a field's declared kind is (or wraps, through `option`/union/array)
/// a `record<...>` that names `table`.
fn kind_targets_table(kind: Option<&Kind>, table: &str) -> bool {
    match kind {
        Some(Kind::Record(tables)) => tables.iter().any(|t| t.to_string() == table),
        Some(Kind::Either(variants)) => {
            variants.iter().any(|v| kind_targets_table(Some(v), table))
        }
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
    let [ast::Projection::Expr { expr, .. }] = stmt.projections.as_slice() else {
        return None;
    };

    match &expr.node {
        ast::Expr::Idiom(idiom) if starts_with_graph(idiom) => {
            crate::analyzer::data::graph::check_graph_idiom(ctx, row_table_name, idiom);
            validate_graph_destructure(ctx, row_table_name, idiom);
            graph_projection_kind(row_table_name, idiom, ctx.schema(), false)
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
        _ => Some(computed_kind(expr, table, ctx)),
    }
}

fn projected_object_kind(
    stmt: &ast::SelectStmt,
    row_table_name: &str,
    table: &TableDef,
    ctx: &mut AnalysisContext<'_>,
) -> Kind {
    let mut fields = BTreeMap::new();

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
                project_expr(
                    expr,
                    alias.as_ref(),
                    stmt,
                    row_table_name,
                    table,
                    ctx,
                    &mut fields,
                );
            }
        }
    }

    object_literal(fields)
}

fn project_expr(
    expr: &ast::Spanned<ast::Expr>,
    alias: Option<&ast::Spanned<String>>,
    stmt: &ast::SelectStmt,
    row_table_name: &str,
    table: &TableDef,
    ctx: &mut AnalysisContext<'_>,
    fields: &mut BTreeMap<String, Kind>,
) {
    let alias_name = alias.map(|a| a.node.clone());

    if let ast::Expr::Idiom(idiom) = &expr.node {
        if starts_with_graph(idiom) {
            crate::analyzer::data::graph::check_graph_idiom(ctx, row_table_name, idiom);
            // A `.{…}` destructure tail selects fields on the resolved graph
            // target; each must exist there (E1002), independent of alias.
            validate_graph_destructure(ctx, row_table_name, idiom);
            // `->likes->post.{title, id}` without an alias fans out into
            // nested per-field arrays.
            if alias_name.is_none() {
                if let Some(outputs) = graph_destructure_output(row_table_name, idiom, ctx.schema())
                {
                    for (segments, kind) in outputs {
                        insert_kind_at_path(fields, &segments, kind);
                    }
                    return;
                }
            }

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
            if let Some(outputs) = destructure_kinds(ctx.schema(), table, &prefix, &selected) {
                match &alias_name {
                    Some(alias) => {
                        let object: BTreeMap<String, Kind> = outputs
                            .into_iter()
                            .map(|(segments, kind)| {
                                (segments.last().cloned().unwrap_or_default(), kind)
                            })
                            .collect();
                        fields.insert(alias.clone(), object_literal(object));
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

        // Idioms with parts we don't project yet (Start/Index/Method/...)
        // still get their invariants checked.
        ctx.with_row_table(ctx.schema().tables.get(&table.name), |ctx| {
            crate::analyzer::expression::check::check_value_expression(ctx, expr);
        });
        fields.insert(
            alias_name.unwrap_or_else(|| slice(ctx.source_text(), expr.span).to_string()),
            Kind::Any,
        );
        return;
    }

    // Computed projection: full expression inference.
    let key = alias_name.unwrap_or_else(|| slice(ctx.source_text(), expr.span).to_string());
    let kind = computed_kind(expr, table, ctx);
    fields.insert(key, kind);
}

fn computed_kind(
    expr: &ast::Spanned<ast::Expr>,
    table: &TableDef,
    ctx: &mut AnalysisContext<'_>,
) -> Kind {
    // An aggregate over a projected column receives the *collected* column,
    // not one row's value — infer it as such so its `array` argument
    // contract is satisfied rather than false-positived.
    if let Some(kind) = column_aggregate_kind(expr, table, ctx) {
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
/// so a scalar column is promoted to `array<column>` before the signature
/// runs. Without this, the per-row `int` reading of `size_bytes` violates
/// the `array` argument contract and false-positives (5002).
///
/// The promotion is deliberately narrow: it fires only for a single bare
/// column argument whose kind is a scalar. A column that is already a
/// collection keeps the ordinary element-wise reading (`array::len(tags)`),
/// and anything but a plain field falls through to normal inference.
fn column_aggregate_kind(
    expr: &ast::Spanned<ast::Expr>,
    table: &TableDef,
    ctx: &mut AnalysisContext<'_>,
) -> Option<Kind> {
    let ast::Expr::Call(call) = &expr.node else {
        return None;
    };
    if !is_column_aggregate(call.path.node.as_str()) {
        return None;
    }
    let [arg] = call.args.as_slice() else {
        return None;
    };
    let ast::Expr::Idiom(idiom) = &arg.node else {
        return None;
    };
    let segments = plain_field_segments(idiom)?;
    let column = kind_for_path(table, &segments)?;
    // Already a collection: the ordinary element-wise contract already fits.
    let base = crate::kinds::literal_base_kind(&column).unwrap_or_else(|| column.clone());
    if matches!(base, Kind::Array(_, _) | Kind::Set(_, _)) {
        return None;
    }
    let collected = Kind::Array(Box::new(column), None);
    Some(crate::analyzer::function::analyze_builtin_function(
        ctx,
        call,
        &[collected],
    ))
}

/// Aggregate functions that reduce a numeric column to one scalar. These all
/// take `array<number>` and return a scalar `number`, so a scalar projected
/// column must be collected first.
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

/// Splits a graph idiom into its leading graph steps and the projected tail.
fn graph_split(
    idiom: &ast::Idiom,
) -> (
    &[ast::Spanned<ast::IdiomPart>],
    &[ast::Spanned<ast::IdiomPart>],
) {
    let boundary = idiom
        .parts
        .iter()
        .position(|part| !matches!(part.node, ast::IdiomPart::Graph { .. }))
        .unwrap_or(idiom.parts.len());
    idiom.parts.split_at(boundary)
}

/// The type of one graph projection (`->likes->post`, `->likes.since`,
/// `->likes->post.{a}` when aliased), wrapped in the traversal's array.
pub(crate) fn graph_projection_kind(
    row_table_name: &str,
    idiom: &ast::Idiom,
    schema: &SchemaIndex,
    materialize_target: bool,
) -> Option<Kind> {
    let (graphs, tail) = graph_split(idiom);

    let projected = if graphs.len() == 1 && !tail.is_empty() {
        // Single hop with a field tail projects off the *edge* table:
        // `->likes.since`.
        let (dir, edge) = single_graph_target(&graphs[0].node)?;
        if !relation_accepts_source(row_table_name, dir, edge, schema) {
            return None;
        }
        let edge_table = schema.tables.get(edge)?;
        let segments = tail_plain_segments(tail)?;
        kind_for_path(edge_table, &segments)?
    } else {
        let target_name = resolve_graph_chain(row_table_name, graphs, schema)?;
        let target = schema.tables.get(&target_name)?;
        if tail.is_empty() {
            if materialize_target {
                object_kind_for_all_fields(target)
            } else {
                Kind::Record(vec![target_name.into()])
            }
        } else if let [part] = tail {
            if let ast::IdiomPart::Destructure(selected) = &part.node {
                let mut object = BTreeMap::new();
                for sub in selected {
                    let segments = plain_field_segments(&sub.node)?;
                    let name = segments.join(".");
                    // A field absent on the target still projects (as `Any`);
                    // validation reports it separately.
                    object.insert(
                        name,
                        resolve_field_path(schema, target, &segments).unwrap_or(Kind::Any),
                    );
                }
                object_literal(object)
            } else {
                let segments = tail_plain_segments(tail)?;
                kind_for_path(target, &segments)?
            }
        } else {
            let segments = tail_plain_segments(tail)?;
            kind_for_path(target, &segments)?
        }
    };

    Some(Kind::Array(Box::new(projected), None))
}

/// `->likes->post.{title, id}` without an alias: one nested output field per
/// selected column, each an array, keyed under the traversal segments.
fn graph_destructure_output(
    row_table_name: &str,
    idiom: &ast::Idiom,
    schema: &SchemaIndex,
) -> Option<Vec<(Vec<String>, Kind)>> {
    let (graphs, tail) = graph_split(idiom);
    let [tail_part] = tail else {
        return None;
    };
    let ast::IdiomPart::Destructure(selected) = &tail_part.node else {
        return None;
    };

    let target_name = resolve_graph_chain(row_table_name, graphs, schema)?;
    let target = schema.tables.get(&target_name)?;
    let graph_segments = graph_segments_of(graphs)?;

    let mut outputs = Vec::new();
    for sub in selected {
        let segments = plain_field_segments(&sub.node)?;
        // A field absent on the target still projects (as `Any`); the field
        // validation runs separately via `validate_graph_destructure`.
        let selected_kind = resolve_field_path(schema, target, &segments).unwrap_or(Kind::Any);
        let mut output_segments = graph_segments.clone();
        output_segments.extend(segments);
        outputs.push((output_segments, Kind::Array(Box::new(selected_kind), None)));
    }
    Some(outputs)
}

/// Output key segments for an unaliased graph projection: `->likes`,
/// `->post`, then any projected tail fields.
fn graph_output_segments(idiom: &ast::Idiom) -> Option<Vec<String>> {
    let (graphs, tail) = graph_split(idiom);
    let mut segments = graph_segments_of(graphs)?;
    segments.extend(tail_plain_segments(tail).unwrap_or_default());
    Some(segments)
}

fn graph_segments_of(graphs: &[ast::Spanned<ast::IdiomPart>]) -> Option<Vec<String>> {
    graphs
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

fn tail_plain_segments(tail: &[ast::Spanned<ast::IdiomPart>]) -> Option<Vec<String>> {
    tail.iter()
        .map(|part| match &part.node {
            ast::IdiomPart::Field(name) => Some(name.clone()),
            _ => None,
        })
        .collect()
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

fn destructure_kinds(
    schema: &SchemaIndex,
    table: &TableDef,
    prefix: &[String],
    selected: &[ast::Spanned<ast::Idiom>],
) -> Option<Vec<(Vec<String>, Kind)>> {
    let mut outputs = Vec::new();
    for sub in selected {
        let sub_segments = plain_field_segments(&sub.node)?;
        let mut segments = prefix.to_vec();
        segments.extend(sub_segments);
        // A field absent on the target still projects (as `Any`);
        // `validate_row_destructure` reports it separately.
        let kind = resolve_field_path(schema, table, &segments).unwrap_or(Kind::Any);
        outputs.push((segments, kind));
    }
    Some(outputs)
}

/// Validates each selected sub-field of a `.{…}` destructure on a graph target
/// (`->friend->user.{name, aeg}`): resolves the traversal's target table and
/// checks each field there (E1002), crossing record links. Silent when the
/// target can't be resolved or isn't in the schema (no false positives).
fn validate_graph_destructure(
    ctx: &mut AnalysisContext<'_>,
    row_table_name: &str,
    idiom: &ast::Idiom,
) {
    let (graphs, tail) = graph_split(idiom);
    let [tail_part] = tail else {
        return;
    };
    let ast::IdiomPart::Destructure(selected) = &tail_part.node else {
        return;
    };
    let schema = ctx.schema();
    let Some(target_name) = resolve_graph_chain(row_table_name, graphs, schema) else {
        return;
    };
    let Some(target) = schema.tables.get(&target_name) else {
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
/// field's kind. It only ever *tightens* a leaf already present under its own
/// name, so aliased projections, computed columns, and fields the WHERE does
/// not mention are left at their schema kind (prove-or-fall-back-to-schema).
///
/// The whole pass is disabled — the schema shape is returned unchanged — under
/// any boundary condition where a projected field cannot be soundly keyed to a
/// plain schema row: no WHERE clause, a `GROUP BY` (rows are groups, not source
/// rows), a `VALUE` projection (P1), or a non-plain-table FROM (subquery /
/// param / graph source).
fn apply_where_narrowing(row_kind: Kind, stmt: &ast::SelectStmt) -> Kind {
    let Some(cond) = &stmt.where_clause else {
        return row_kind;
    };
    if stmt.group.is_some() || stmt.value {
        return row_kind;
    }
    // A plain schema-object row is required to key row fields by name.
    match stmt.from.first().map(|from| &from.node) {
        Some(ast::Expr::Table(_) | ast::Expr::RecordId { .. }) => {}
        _ => return row_kind,
    }
    let Kind::Literal(KindLiteral::Object(mut fields)) = row_kind else {
        return row_kind;
    };
    for effect in crate::analyzer::flow::narrow::where_effects(&cond.node) {
        narrow_kind_at_path(&mut fields, &effect.fields, &effect.narrowing);
    }
    object_literal(fields)
}

/// Tightens the leaf at `segments` in a projected object literal, if that exact
/// path is present. Tighten-only: an absent key (a predicate on a non-projected
/// or aliased field) is skipped, and a refinement that does not tighten the
/// current leaf leaves it unchanged.
fn narrow_kind_at_path(
    fields: &mut BTreeMap<String, Kind>,
    segments: &[String],
    narrowing: &crate::analyzer::flow::narrow::Narrowing,
) {
    let Some((first, rest)) = segments.split_first() else {
        return;
    };
    let Some(kind) = fields.get_mut(first) else {
        return;
    };
    if rest.is_empty() {
        if let Some(narrowed) = crate::analyzer::flow::narrow::narrow_kind(kind, narrowing) {
            *kind = narrowed;
        }
        return;
    }
    if let Kind::Literal(KindLiteral::Object(child_fields)) = kind {
        narrow_kind_at_path(child_fields, rest, narrowing);
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
    let ast::Expr::Literal(ast::Literal::Int(value)) = &limit.node else {
        return None;
    };
    u64::try_from(*value).ok()
}

fn slice(text: &str, range: surrealguard_syntax::span::ByteRange) -> &str {
    text[range.start() as usize..range.end() as usize].trim()
}

// ---------------------------------------------------------------------------
// Shared schema-typed kind builders (used by both worlds and by mutations)
// ---------------------------------------------------------------------------

/// The closed object type of a table's full schema.
pub(crate) fn object_kind_for_all_fields(table: &TableDef) -> Kind {
    object_kind_for_field_prefix(table, &[])
}

fn object_kind_for_field_prefix(table: &TableDef, prefix: &[String]) -> Kind {
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
            object_kind_for_field_prefix(table, &child_prefix)
        } else {
            field.kind.clone().unwrap_or(Kind::Any)
        };
        fields.insert(segment, kind);
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
        return Some(object_kind_for_field_prefix(table, segments));
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
    let Some((head, rest)) = segments.split_first() else {
        return None;
    };
    let head_kind = table
        .fields
        .get(head)
        .and_then(|field| field.kind.clone())
        .or_else(|| table.implicit_field_kind(head));
    match head_kind {
        Some(kind @ Kind::Record(_)) if rest.is_empty() => Some(kind),
        Some(Kind::Record(_)) => Some(Kind::Any),
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
        if let Some(targets) = record_link_targets_at(table, prefix) {
            return Some(resolve_across_link(schema, &targets, rest));
        }
    }
    kind_for_path(table, segments)
}

/// The linked tables when `prefix` names a record link on `table` — a declared
/// `record<...>` field (leaf) or, for a single head segment, an implicit
/// `id`/`in`/`out`. `None` when `prefix` is not a record link, so callers keep
/// resolving within the same table.
fn record_link_targets_at(
    table: &TableDef,
    prefix: &[String],
) -> Option<Vec<surrealdb_types::Table>> {
    if let Some(field) = table.fields.get(&prefix.join(".")) {
        return match &field.kind {
            Some(Kind::Record(targets)) => Some(targets.clone()),
            _ => None,
        };
    }
    if let [head] = prefix {
        if let Some(Kind::Record(targets)) = table.implicit_field_kind(head) {
            return Some(targets);
        }
    }
    None
}

/// Resolves `rest` across a record link to `targets`, widening to `Kind::Any`
/// whenever the answer isn't provable: an empty (`record<>`) or unknown target,
/// or variants that disagree on the remainder's kind.
fn resolve_across_link(
    schema: &SchemaIndex,
    targets: &[surrealdb_types::Table],
    rest: &[String],
) -> Kind {
    if targets.is_empty() {
        return Kind::Any;
    }
    let mut resolved: Option<Kind> = None;
    for target in targets {
        let Some(table) = schema.tables.get(&target.to_string()) else {
            return Kind::Any;
        };
        let Some(kind) = resolve_field_path(schema, table, rest) else {
            return Kind::Any;
        };
        match &resolved {
            None => resolved = Some(kind),
            Some(prev) if *prev == kind => {}
            Some(_) => return Kind::Any,
        }
    }
    resolved.unwrap_or(Kind::Any)
}

/// Validates a (possibly link-crossing) field path against the schema, emitting
/// `code` (E1002) at the table where a segment is genuinely absent. When the
/// path crosses a record link into a single *known* table, validation continues
/// there — so `team.badfield` reports against `team`, with its `DEFINE TABLE`
/// note. A `record<>`, an unknown/dangling target (already E1001 elsewhere), or
/// a union link (multiple targets) suppresses the finding: reporting those
/// would double-report or risk a false positive.
pub(crate) fn validate_field_path(
    ctx: &mut AnalysisContext<'_>,
    table: &TableDef,
    segments: &[String],
    span: surrealguard_syntax::span::ByteRange,
    code: u16,
) {
    for split in 1..segments.len() {
        let (prefix, rest) = segments.split_at(split);
        if let Some(targets) = record_link_targets_at(table, prefix) {
            let [only] = targets.as_slice() else {
                return;
            };
            if let Some(linked) = ctx.schema().tables.get(&only.to_string()) {
                validate_field_path(ctx, linked, rest, span, code);
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

/// Whether `prefix` names a concrete declared *leaf* field on `table` that is
/// not a resolvable known record link — traversing past it is unprovable, so a
/// field finding on the remainder would be unsound. A nested-object prefix (no
/// leaf field at `prefix`, only deeper declarations) and a prefix that names no
/// field at all are both *not* opaque: their absence/children are enumerable.
fn field_is_opaque_boundary(table: &TableDef, prefix: &[String]) -> bool {
    match table.fields.get(&prefix.join(".")) {
        Some(field) => !matches!(&field.kind, Some(Kind::Record(_))),
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

    fn diagnostics_for(schema: &SchemaIndex, query: &str) -> Vec<surrealguard_diagnostics::Finding> {
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
        diagnostics_for(schema, query)
            .iter()
            .any(|finding| finding.code().number() == 4003)
    }

    #[test]
    fn only_whole_table_needs_a_single_row_guarantee() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;",
        );

        // Unfiltered `FROM ONLY <table>` has no cardinality guarantee: 4003.
        assert!(fires_4003(&schema, "SELECT * FROM ONLY person;"));
        // A WHERE clause enforces single-row cardinality at runtime: no 4003.
        assert!(!fires_4003(
            &schema,
            "SELECT * FROM ONLY person WHERE name = 'A';"
        ));
        // LIMIT 1 still exempts it.
        assert!(!fires_4003(&schema, "SELECT * FROM ONLY person LIMIT 1;"));
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

        let kind = analyze(&schema, "SELECT * FROM ONLY person:one;");
        let fields = object_fields(&kind);
        assert_eq!(fields["name"], Kind::String);
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
    fn graph_projection_without_alias_nests_under_traversal_segments() {
        let schema = schema_from(
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string;\nDEFINE FIELD age ON user TYPE int;\nDEFINE TABLE friend TYPE RELATION IN person OUT user;",
        );

        let kind = analyze(&schema, "SELECT ->friend->user.{name, age} FROM person;");
        let fields = object_fields(array_element(&kind));

        let friend = object_fields(&fields["->friend"]);
        let user = object_fields(&friend["->user"]);
        assert_eq!(user["name"], Kind::Array(Box::new(Kind::String), None));
        assert_eq!(user["age"], Kind::Array(Box::new(Kind::Int), None));
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

        let (_, diagnostics) =
            analyze_diagnostics(&schema, "SELECT count() FROM employee_of WHERE status = 'active';");
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
            "SELECT VALUE math::sum(size_bytes) FROM file WHERE status = 'active';",
        );
        // The column is collected into `array<int>`, so the `array`
        // argument contract holds — no 5002 false positive.
        assert!(
            !codes(&diagnostics).contains(&5002),
            "aggregate over a scalar column should not trip the argument check: {:?}",
            codes(&diagnostics)
        );
        assert_eq!(kind, Kind::Array(Box::new(Kind::Number), None));
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
    fn union_record_link_resolves_a_field_common_to_every_variant() {
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

        // `purrs` exists only on `cat` -> widen to Any (never invent), and no
        // 1002 (a union is too ambiguous to report without a false positive).
        let (kind, diagnostics) = analyze_diagnostics(&schema, "SELECT VALUE pet.purrs FROM owner;");
        assert_eq!(kind, Kind::Array(Box::new(Kind::Any), None));
        assert!(
            !codes(&diagnostics).contains(&1002),
            "a union-link traversal must not emit 1002: {:?}",
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

    #[test]
    fn graph_destructure_types_each_selected_field() {
        let schema = person_friend_user_schema();

        let (kind, diagnostics) =
            analyze_diagnostics(&schema, "SELECT ->friend->user.{name, age} FROM person;");
        let fields = object_fields(array_element(&kind));
        let friend = object_fields(&fields["->friend"]);
        let user = object_fields(&friend["->user"]);
        assert_eq!(user["name"], Kind::Array(Box::new(Kind::String), None));
        assert_eq!(user["age"], Kind::Array(Box::new(Kind::Int), None));
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
        let user = object_fields(&friend["->user"]);
        assert_eq!(user["name"], Kind::Array(Box::new(Kind::String), None));

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
        assert!(!finding.related().is_empty(), "expected user's definition note");
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
            finding.message().contains("`person` has no field `profile.nope`"),
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
        assert_eq!(object_fields(array_element(&baseline))["email"], option_string());

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
    fn where_or_narrows_nothing() {
        let schema = narrowing_schema();
        let kind = analyze(
            &schema,
            "SELECT role FROM user WHERE role = 'admin' OR role = 'mod';",
        );
        assert_eq!(object_fields(array_element(&kind))["role"], Kind::String);
    }

    #[test]
    fn group_by_disables_narrowing() {
        let schema = narrowing_schema();
        let kind = analyze(
            &schema,
            "SELECT email FROM user WHERE email != NONE GROUP BY country;",
        );
        assert_eq!(object_fields(array_element(&kind))["email"], option_string());
    }

    #[test]
    fn value_projection_disables_narrowing() {
        let schema = narrowing_schema();
        let kind = analyze(&schema, "SELECT VALUE email FROM user WHERE email != NONE;");
        assert_eq!(kind, Kind::Array(Box::new(option_string()), None));
    }

    #[test]
    fn aliased_projection_is_not_narrowed() {
        // `email AS e` does not match by identity in P1 — the projected `e`
        // keeps its schema kind.
        let schema = narrowing_schema();
        let kind = analyze(&schema, "SELECT email AS e FROM user WHERE email != NONE;");
        assert_eq!(object_fields(array_element(&kind))["e"], option_string());
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
        let kind = analyze(
            &schema,
            "SELECT email, age FROM user WHERE email != NONE;",
        );
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
        assert_eq!(object_fields(array_element(&kind))["email"], option_string());
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
}
