//! Shared mutation response-type logic.
//!
//! Pure utility module with no entry point: each of the six mutation
//! analyzers resolves its own target from its own lowered statement and
//! calls [`response_kind_for_target`] for the part that is genuinely
//! identical once a table is known — the parsed `RETURN` mode plus the
//! `ONLY` wrapper. The result is a plain `Kind`; undeterminable cases are
//! `Kind::Any` poison values.

use std::collections::BTreeMap;

use surrealdb_types::{Kind, KindLiteral};
use surrealguard_syntax::ast;
use surrealguard_syntax::span::ByteRange;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::expression::infer::{infer_expression_fact, plain_field_segments};
use crate::schema::TableDef;

/// Walks the expression positions a statement carries beyond its response
/// shape — WHERE conditions and data-clause payloads. The kinds are
/// discarded; the walk exists so findings inside those expressions are
/// emitted (function misuse in a `WHERE` is as real as in a projection).
pub(crate) fn analyze_expression_positions(
    ctx: &mut AnalysisContext<'_>,
    data: Option<&ast::DataClause>,
    where_clause: Option<&ast::Spanned<ast::Expr>>,
    table: Option<&str>,
) {
    analyze_expression_positions_for(ctx, data, where_clause, table, false);
}

/// `creating` distinguishes CREATE/INSERT/RELATE (where READONLY fields
/// are legitimately written) from UPDATE/UPSERT (2025).
pub(crate) fn analyze_expression_positions_for(
    ctx: &mut AnalysisContext<'_>,
    data: Option<&ast::DataClause>,
    where_clause: Option<&ast::Spanned<ast::Expr>>,
    table: Option<&str>,
    creating: bool,
) {
    let row_table = table.and_then(|name| ctx.schema().tables.get(name));
    ctx.with_row_table(row_table, |ctx| {
        if let Some(cond) = where_clause {
            infer_expression_fact(cond, ctx);
            crate::analyzer::expression::check::check_value_expression(ctx, cond);
            if let Some(table) = row_table {
                crate::analyzer::data::check_expression_field_paths(ctx, table, cond, 1002);
            }
        }
        match data {
            Some(ast::DataClause::Set(assignments)) => {
                check_duplicate_targets(ctx, assignments);
                for assignment in assignments {
                    infer_expression_fact(&assignment.value, ctx);
                    crate::analyzer::expression::check::check_value_expression(
                        ctx,
                        &assignment.value,
                    );
                    if let Some(table) = row_table {
                        check_assignment_target(ctx, table, &assignment.target);
                        check_assignment_value(ctx, table, assignment);
                        check_field_write_flags(ctx, table, &assignment.target, creating);
                    }
                }
            }
            Some(ast::DataClause::Unset(idioms)) => {
                if let Some(table) = row_table {
                    for idiom in idioms {
                        if let Some(segments) = plain_field_segments(&idiom.node) {
                            crate::analyzer::data::check_field_path(
                                ctx, table, &segments, idiom.span, 1002,
                            );
                        }
                    }
                }
            }
            Some(
                ast::DataClause::Content(expr)
                | ast::DataClause::Merge(expr)
                | ast::DataClause::Replace(expr),
            ) => {
                infer_expression_fact(expr, ctx);
                if let Some(table) = row_table {
                    check_payload_object_keys(ctx, table, expr);
                }
            }
            Some(ast::DataClause::Patch(expr)) => {
                infer_expression_fact(expr, ctx);
                check_patch_operations(ctx, expr);
            }
            Some(ast::DataClause::Single(expr)) => {
                infer_expression_fact(expr, ctx);
            }
            Some(ast::DataClause::Partial(_)) | None => {}
        }
    });
}

/// A required field (non-optional declared type, no DEFAULT) must be
/// provided when a row is created (2034).
pub fn check_required_fields(
    ctx: &mut AnalysisContext<'_>,
    table: &TableDef,
    provided: &[String],
    anchor: surrealguard_syntax::span::ByteRange,
) {
    for (path, field) in &table.fields {
        // Only top-level fields are directly required; nested paths are
        // satisfied through their parent object.
        if field.path.len() != 1 || path == "id" || field.has_default {
            continue;
        }
        let Some(kind) = &field.kind else {
            continue;
        };
        let optional = match kind {
            Kind::Either(variants) => variants
                .iter()
                .any(|v| matches!(v, Kind::None | Kind::Null)),
            Kind::None | Kind::Null | Kind::Any => true,
            _ => false,
        };
        if optional || provided.iter().any(|name| name == path) {
            continue;
        }
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), anchor);
        ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                span,
                2034,
                format!("`{path}` must be set when creating a `{}`", table.name),
            )
            .with_help(format!(
                "`{path}` is `{}` with no `DEFAULT`, so every create must provide it",
                crate::render_kind(kind)
            ))
            .with_related(
                field.name_span.clone(),
                format!("`{path}` is defined here"),
            ),
        );
    }
}

/// The top-level field names a data clause provides, or `None` when the
/// clause's key set is not statically known.
///
/// The distinction is the whole contract of 2034: "provides nothing"
/// (`CREATE person;`, `CONTENT {}`) is evidence that a required field is
/// missing, but "shape unknown" (`CONTENT $payload`) is not. Conflating the
/// two reported every required field as missing on a perfectly valid
/// parameterized create — an error, so it also refused to write the
/// registry for the whole project.
pub fn provided_field_names(
    ctx: &AnalysisContext<'_>,
    data: Option<&ast::DataClause>,
) -> Option<Vec<String>> {
    match data {
        // No data clause at all: nothing is provided.
        None => Some(Vec::new()),
        Some(ast::DataClause::Set(assignments)) => Some(
            assignments
                .iter()
                .filter_map(|assignment| {
                    plain_field_segments(&assignment.target.node)
                        .and_then(|segments| segments.first().cloned())
                })
                .collect(),
        ),
        Some(
            ast::DataClause::Content(expr)
            | ast::DataClause::Replace(expr)
            | ast::DataClause::Merge(expr),
        ) => payload_field_names(ctx, expr),
        // PATCH/UNSET/an unlowered clause: no key set to read.
        Some(_) => None,
    }
}

/// The top-level keys a payload expression provides: an object literal's
/// keys, or those of a `LET`-bound parameter the analyzer has already typed
/// as a closed object. `None` for every other payload — an unbound
/// `$payload`, a function call, a subquery — whose keys are unknowable here.
pub fn payload_field_names(
    ctx: &AnalysisContext<'_>,
    expr: &ast::Spanned<ast::Expr>,
) -> Option<Vec<String>> {
    match &expr.node {
        ast::Expr::Object(fields) => Some(fields.iter().map(|(key, _)| key.node.clone()).collect()),
        ast::Expr::Param(name) => match ctx.env().let_fact(name)?.kind.as_ref()? {
            Kind::Literal(KindLiteral::Object(fields)) => {
                Some(fields.keys().cloned().collect::<Vec<_>>())
            }
            _ => None,
        },
        _ => None,
    }
}

/// The rows one `INSERT` payload expression carries.
///
/// The array form (`INSERT INTO t [{…}, {…}]`) is a *single* payload
/// expression holding one object per row; every other form is itself the
/// one row. Each element is an independent record, so key, value-kind and
/// required-field checks all run per row — this is the one place that
/// distinction is made, so no caller can forget the array form.
pub fn insert_payload_rows(value: &ast::Spanned<ast::Expr>) -> Vec<&ast::Spanned<ast::Expr>> {
    match &value.node {
        ast::Expr::Array(rows) => rows.iter().collect(),
        _ => vec![value],
    }
}

/// A write to a whole table with no WHERE touches every row — legal, and
/// occasionally intended, but worth a deliberate look (7009).
pub fn check_whole_table_write(
    ctx: &mut AnalysisContext<'_>,
    target: Option<&ast::Spanned<ast::Expr>>,
    where_clause: Option<&ast::Spanned<ast::Expr>>,
) {
    if where_clause.is_some() {
        return;
    }
    let Some(target) = target else {
        return;
    };
    if let ast::Expr::Table(name) = &target.node {
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), target.span);
        ctx.emit(surrealguard_diagnostics::catalog::finding(
            span,
            7009,
            format!(
                "this writes every row of `{}`; add WHERE or a record id",
                name.node
            ),
        ));
    }
}

/// Relation rows need `in` and `out`; creating one without them makes an
/// edge connected to nothing (4019).
pub fn check_relation_write(
    ctx: &mut AnalysisContext<'_>,
    target: Option<&ast::Spanned<ast::Expr>>,
    data: Option<&ast::DataClause>,
) {
    let Some(target) = target else {
        return;
    };
    let Some(name) = source_table_name(Some(target)) else {
        return;
    };
    let is_relation = ctx
        .schema()
        .tables
        .get(&name)
        .is_some_and(|table| table.relation.is_some());
    if !is_relation {
        return;
    }
    let provides = |key: &str| match data {
        Some(ast::DataClause::Set(assignments)) => assignments.iter().any(|assignment| {
            crate::analyzer::expression::infer::plain_field_segments(&assignment.target.node)
                .is_some_and(|segments| segments == [key])
        }),
        Some(ast::DataClause::Content(expr) | ast::DataClause::Replace(expr)) => {
            matches!(&expr.node, ast::Expr::Object(fields)
                if fields.iter().any(|(k, _)| k.node == key))
        }
        _ => false,
    };
    if !(provides("in") && provides("out")) {
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), target.span);
        ctx.emit(surrealguard_diagnostics::catalog::finding(
            span,
            4019,
            format!("`{name}` is a relation; use RELATE (or provide `in` and `out`)"),
        ));
    }
}

/// INSERT's variant of the relation contract (4019).
pub fn check_relation_insert(
    ctx: &mut AnalysisContext<'_>,
    target: Option<&ast::Spanned<ast::Expr>>,
    data: &ast::InsertData,
) {
    let Some(target) = target else {
        return;
    };
    let Some(name) = source_table_name(Some(target)) else {
        return;
    };
    let is_relation = ctx
        .schema()
        .tables
        .get(&name)
        .is_some_and(|table| table.relation.is_some());
    if !is_relation {
        return;
    }
    let object_has = |expr: &ast::Spanned<ast::Expr>, key: &str| {
        matches!(&expr.node, ast::Expr::Object(fields)
            if fields.iter().any(|(k, _)| k.node == key))
    };
    let provided = match data {
        ast::InsertData::Values(values) => values
            .iter()
            .flat_map(|value| insert_payload_rows(value))
            .all(|row| object_has(row, "in") && object_has(row, "out")),
        ast::InsertData::Rows { rows, .. } => rows.iter().all(|row| {
            let has = |key: &str| {
                row.iter().any(|(column, _)| {
                    crate::analyzer::expression::infer::plain_field_segments(&column.node)
                        .is_some_and(|segments| segments == [key])
                })
            };
            has("in") && has("out")
        }),
        _ => false,
    };
    if !provided {
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), target.span);
        ctx.emit(surrealguard_diagnostics::catalog::finding(
            span,
            4019,
            format!("`{name}` is a relation; use RELATE (or provide `in` and `out`)"),
        ));
    }
}

/// `CREATE ... RETURN BEFORE` always returns NONE — there is no before
/// state at creation (4020).
pub fn check_return_before_on_create(
    ctx: &mut AnalysisContext<'_>,
    ret: Option<&ast::Spanned<ast::ReturnMode>>,
) {
    if let Some(ret) = ret {
        if matches!(ret.node, ast::ReturnMode::Before) {
            let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), ret.span);
            ctx.emit(surrealguard_diagnostics::catalog::finding(
                span,
                4020,
                "RETURN BEFORE on CREATE is always NONE; there is no before state".to_string(),
            ));
        }
    }
}

/// PATCH operations must be well-formed JSON-Patch (2033): known ops and
/// `/`-prefixed paths. Only constant payloads are checkable.
fn check_patch_operations(ctx: &mut AnalysisContext<'_>, expr: &ast::Spanned<ast::Expr>) {
    const OPS: &[&str] = &["add", "remove", "replace", "move", "copy", "test", "change"];
    let ast::Expr::Array(operations) = &expr.node else {
        return;
    };
    for operation in operations {
        let ast::Expr::Object(fields) = &operation.node else {
            continue;
        };
        for (key, value) in fields {
            let ast::Expr::Literal(ast::Literal::String(text)) = &value.node else {
                continue;
            };
            let problem = match key.node.as_str() {
                "op" if !OPS.contains(&text.as_str()) => {
                    Some(format!("`{text}` is not a PATCH operation"))
                }
                "path" if !text.starts_with('/') => {
                    Some(format!("PATCH paths start with `/`; found `{text}`"))
                }
                _ => None,
            };
            if let Some(message) = problem {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), value.span);
                ctx.emit(surrealguard_diagnostics::catalog::finding(
                    span, 2033, message,
                ));
            }
        }
    }
}

/// READONLY fields are written only at creation (2025); computed
/// (VALUE-clause) fields are overwritten on every write (2026).
fn check_field_write_flags(
    ctx: &mut AnalysisContext<'_>,
    table: &TableDef,
    target: &ast::Spanned<ast::Idiom>,
    creating: bool,
) {
    let Some(segments) = plain_field_segments(&target.node) else {
        return;
    };
    let Some(field) = table.fields.get(&segments.join(".")) else {
        return;
    };
    let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), target.span);
    let path = segments.join(".");
    if field.readonly && !creating {
        ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                span,
                2025,
                format!("`{path}` can't be changed after creation"),
            )
            .with_help(format!("`{path}` is READONLY; set it once, when the row is created"))
            .with_related(field.name_span.clone(), format!("`{path}` is defined READONLY here")),
        );
        return;
    }
    if field.computed {
        ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                span,
                2026,
                format!("this write to `{path}` is discarded"),
            )
            .with_help(format!(
                "`{path}` is computed by its VALUE clause, which overwrites any assigned value"
            ))
            .with_related(field.name_span.clone(), format!("`{path}` is defined here")),
        );
    }
}

/// `SET age = 1, age = 2` — the later assignment silently wins (4010).
fn check_duplicate_targets(ctx: &mut AnalysisContext<'_>, assignments: &[ast::Assignment]) {
    let mut seen = std::collections::BTreeMap::new();
    for assignment in assignments {
        if !matches!(assignment.op.node, ast::AssignOp::Assign) {
            continue;
        }
        let Some(segments) = plain_field_segments(&assignment.target.node) else {
            continue;
        };
        let key = segments.join(".");
        if seen.insert(key.clone(), assignment.target.span).is_some() {
            let span = surrealguard_syntax::span::SourceSpan::new(
                ctx.source().clone(),
                assignment.target.span,
            );
            ctx.emit(surrealguard_diagnostics::catalog::finding(
                span,
                4010,
                format!("`{key}` is assigned more than once; the last assignment wins"),
            ));
        }
    }
}

/// ONLY on a whole-table target is a deterministic runtime error for
/// row-iterating mutations (4003); record ids and CREATE (always one row)
/// are fine.
pub fn check_only_on_table(
    ctx: &mut AnalysisContext<'_>,
    only: bool,
    target: Option<&ast::Spanned<ast::Expr>>,
) {
    if !only {
        return;
    }
    let Some(target) = target else {
        return;
    };
    if matches!(target.node, ast::Expr::Table(_)) {
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), target.span);
        ctx.emit(surrealguard_diagnostics::catalog::finding(
            span,
            4003,
            "ONLY on a whole table needs a record id target".to_string(),
        ));
    }
}

/// `SET target = value`: the written value must inhabit the field's
/// declared type (2001); compound operators check as operator
/// applications against the field's kind (2004); `id` is not writable
/// (7011).
fn check_assignment_value(
    ctx: &mut AnalysisContext<'_>,
    table: &TableDef,
    assignment: &ast::Assignment,
) {
    let Some(segments) = plain_field_segments(&assignment.target.node) else {
        return;
    };
    if segments == ["id"] {
        let span = surrealguard_syntax::span::SourceSpan::new(
            ctx.source().clone(),
            assignment.target.span,
        );
        ctx.emit(surrealguard_diagnostics::catalog::finding(
            span,
            7011,
            "record ids are immutable; `id` is set at creation".to_string(),
        ));
        return;
    }
    // Compound assignment is an operator application: `age += x` must make
    // sense as `age + x`.
    if !matches!(assignment.op.node, ast::AssignOp::Assign) {
        let (Some(field_kind), Some(value_kind)) = (
            crate::analyzer::data::select::kind_for_path(table, &segments),
            infer_expression_fact(&assignment.value, ctx).kind,
        ) else {
            return;
        };
        if field_kind == Kind::Any || value_kind == Kind::Any {
            return;
        }
        let op = match assignment.op.node {
            ast::AssignOp::Add => ast::BinaryOp::Add,
            ast::AssignOp::Sub => ast::BinaryOp::Sub,
            _ => return,
        };
        if crate::analyzer::expression::infer::binary_result_kind(&op, &field_kind, &value_kind)
            .is_none()
        {
            let span = surrealguard_syntax::span::SourceSpan::new(
                ctx.source().clone(),
                assignment.value.span,
            );
            let op_text = if matches!(op, ast::BinaryOp::Add) {
                "+="
            } else {
                "-="
            };
            let path = segments.join(".");
            let mut finding = surrealguard_diagnostics::catalog::finding(
                span,
                2004,
                format!("`{op_text}` can't combine a `{}` and a `{}`", crate::render_kind(&field_kind), crate::render_kind(&value_kind)),
            )
            .with_help(format!(
                "`{path}` is `{}`; `{op_text}` needs a right-hand value that combines with it",
                crate::render_kind(&field_kind)
            ));
            if let Some(def) = table.fields.get(&path) {
                finding =
                    finding.with_related(def.name_span.clone(), format!("`{path}` is defined here"));
            }
            ctx.emit(finding);
        }
        return;
    }
    let Some(field_kind) = crate::analyzer::data::select::kind_for_path(table, &segments) else {
        return;
    };
    // An unbound parameter here is constrained to the field's kind;
    // bound ones check like any value.
    if let ast::Expr::Param(param) = &assignment.value.node {
        if ctx.env().let_fact(param).is_none() {
            let span = surrealguard_syntax::span::SourceSpan::new(
                ctx.source().clone(),
                assignment.value.span,
            );
            ctx.constrain_param(param, span, field_kind.clone(), None);
            return;
        }
    }
    let value_fact = infer_expression_fact(&assignment.value, ctx);
    let Some(value_kind) = value_fact.kind else {
        return;
    };
    // Against a scalar-literal field (`'active' | 'inactive'`), a written
    // constant must be compared as the literal it *is*: inference widens
    // `'bogus'` to `string`, which the field's literal union has to accept
    // (a `string` can't be shown to fall outside it), so the wrong value
    // would slip through. Recovering the exact literal restores the equality
    // check. Bounded to literal-constrained fields — everywhere else a
    // literal is assignable exactly where its base is, so this changes nothing.
    let value_kind = value_fact
        .value
        .as_ref()
        .filter(|_| crate::kinds::constrains_scalar_literals(&field_kind))
        .and_then(crate::kinds::scalar_value_literal_kind)
        .unwrap_or(value_kind);
    if value_kind == Kind::Any || crate::kinds::kind_is_assignable_to(&value_kind, &field_kind) {
        return;
    }
    let span =
        surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), assignment.value.span);
    let field = segments.join(".");
    // One contract: the value must inhabit the field's declared type. NONE
    // gets the actionable variant of the message, not its own code.
    let mut finding = if matches!(value_kind, Kind::None | Kind::Null) {
        surrealguard_diagnostics::catalog::finding(
            span,
            2001,
            format!("`{field}` is not optional, so it can't be set to {}", crate::render_kind(&value_kind)),
        )
        .with_help(format!(
            "declare it `option<{}>`, or coalesce with `?? <value>`",
            crate::render_kind(&field_kind)
        ))
    } else {
        surrealguard_diagnostics::catalog::finding(
            span,
            2001,
            format!("`{field}` is declared `{}`, but this value is `{}`", crate::render_kind(&field_kind), crate::render_kind(&value_kind)),
        )
    };
    if let Some(def) = table.fields.get(&field) {
        finding = finding.with_related(def.name_span.clone(), format!("`{field}` is defined here"));
    }
    ctx.emit(finding);
}

/// `SET target = ...`: the target must be a declared field path (1004).
fn check_assignment_target(
    ctx: &mut AnalysisContext<'_>,
    table: &TableDef,
    target: &ast::Spanned<ast::Idiom>,
) {
    if let Some(segments) = plain_field_segments(&target.node) {
        crate::analyzer::data::check_field_path(ctx, table, &segments, target.span, 1002);
    }
}

/// `CONTENT`/`MERGE`/`REPLACE` object literals (and INSERT object
/// payloads): each key path must be a declared field (1005). Nested
/// objects check their dotted paths.
pub fn check_payload_object_keys(
    ctx: &mut AnalysisContext<'_>,
    table: &TableDef,
    expr: &ast::Spanned<ast::Expr>,
) {
    fn walk(
        ctx: &mut AnalysisContext<'_>,
        table: &TableDef,
        expr: &ast::Spanned<ast::Expr>,
        prefix: &[String],
    ) {
        let ast::Expr::Object(fields) = &expr.node else {
            return;
        };
        for (key, value) in fields {
            if key.node == "id" {
                continue;
            }
            let mut segments = prefix.to_vec();
            segments.push(key.node.clone());
            let Some(field_kind) = crate::analyzer::data::select::kind_for_path(table, &segments)
            else {
                crate::analyzer::data::check_field_path(ctx, table, &segments, key.span, 1002);
                continue;
            };
            if matches!(value.node, surrealguard_syntax::ast::Expr::Object(_)) {
                // The path resolves; descend for nested keys under it.
                walk(ctx, table, value, &segments);
                continue;
            }
            if let surrealguard_syntax::ast::Expr::Param(param) = &value.node {
                if ctx.env().let_fact(param).is_none() {
                    let span = surrealguard_syntax::span::SourceSpan::new(
                        ctx.source().clone(),
                        value.span,
                    );
                    ctx.constrain_param(param, span, field_kind.clone(), None);
                    continue;
                }
            }
            let Some(value_kind) = infer_expression_fact(value, ctx).kind else {
                continue;
            };
            if value_kind != Kind::Any
                && !crate::kinds::kind_is_assignable_to(&value_kind, &field_kind)
            {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), value.span);
                let path = segments.join(".");
                let mut finding = surrealguard_diagnostics::catalog::finding(
                    span,
                    2001,
                    format!(
                        "`{path}` is declared `{}`, but this value is `{}`",
                        crate::render_kind(&field_kind),
                        crate::render_kind(&value_kind)
                    ),
                );
                if let Some(def) = table.fields.get(&path) {
                    finding = finding
                        .with_related(def.name_span.clone(), format!("`{path}` is defined here"));
                }
                ctx.emit(finding);
            }
        }
    }
    walk(ctx, table, expr, &[]);
}

/// Builds the response type for a mutation once its target `table` is
/// resolved: `RETURN` mode decides the row type, `ONLY` decides whether the
/// row is wrapped in an array.
pub fn response_kind_for_target(
    only: bool,
    ret: Option<&ast::Spanned<ast::ReturnMode>>,
    table: &TableDef,
    ctx: &mut AnalysisContext<'_>,
) -> Kind {
    let row = match ret.map(|r| &r.node) {
        // `RETURN NONE` yields no rows at all.
        Some(ast::ReturnMode::None) => {
            return if only {
                Kind::None
            } else {
                Kind::Array(Box::new(Kind::Any), Some(0))
            };
        }
        Some(ast::ReturnMode::Null) => Kind::Null,
        Some(ast::ReturnMode::Diff) => patch_operations_kind(),
        Some(ast::ReturnMode::Fields(projections)) => fields_row_kind(projections, table, ctx),
        // BEFORE/AFTER/default all produce full rows. (BEFORE on CREATE is
        // arguably `none` — statement-specific refinement is tracked with
        // the DELETE default-shape question, pending verification against
        // real SurrealDB behavior.)
        Some(ast::ReturnMode::Before | ast::ReturnMode::After) | None => {
            if table.fields.is_empty() {
                return Kind::Any;
            }
            crate::analyzer::data::select::object_kind_for_all_fields(table)
        }
    };

    if only {
        row
    } else {
        Kind::Array(Box::new(row), None)
    }
}

/// `RETURN DIFF`: one patch-operation list per row.
fn patch_operations_kind() -> Kind {
    let mut fields = BTreeMap::new();
    fields.insert("op".to_string(), Kind::String);
    fields.insert("path".to_string(), Kind::String);
    fields.insert("value".to_string(), Kind::Any);
    Kind::Array(Box::new(Kind::Literal(KindLiteral::Object(fields))), None)
}

/// `RETURN <fields>`: the projected row object.
fn fields_row_kind(
    projections: &[ast::Projection],
    table: &TableDef,
    ctx: &mut AnalysisContext<'_>,
) -> Kind {
    if projections
        .iter()
        .any(|projection| matches!(projection, ast::Projection::Wildcard(_)))
    {
        return crate::analyzer::data::select::object_kind_for_all_fields(table);
    }

    let mut fields = BTreeMap::new();
    for projection in projections {
        match projection {
            ast::Projection::Wildcard(_) => {}
            ast::Projection::Partial(partial) => {
                fields.insert(
                    slice(ctx.source_text(), partial.span).to_string(),
                    Kind::Any,
                );
            }
            ast::Projection::Expr { expr, alias } => {
                let alias_name = alias.as_ref().map(|a| a.node.clone());
                if let ast::Expr::Idiom(idiom) = &expr.node {
                    if let Some(segments) = plain_field_segments(idiom) {
                        match crate::analyzer::data::select::kind_for_path(table, &segments) {
                            Some(kind) => match alias_name {
                                Some(alias) => {
                                    fields.insert(alias, kind);
                                }
                                None => crate::analyzer::data::select::insert_kind_at_path(
                                    &mut fields,
                                    &segments,
                                    kind,
                                ),
                            },
                            None => {
                                crate::analyzer::data::check_field_path(
                                    ctx, table, &segments, expr.span, 1002,
                                );
                                fields.insert(
                                    alias_name.unwrap_or_else(|| segments.join(".")),
                                    Kind::Any,
                                );
                            }
                        }
                        continue;
                    }
                }
                // Computed return expression: full inference.
                let row_table = ctx.schema().tables.get(&table.name);
                let kind = ctx.with_row_table(row_table, |ctx| {
                    infer_expression_fact(expr, ctx).kind.unwrap_or(Kind::Any)
                });
                // A mutation's RETURN projections are named by the same rule
                // a SELECT's are (`RETURN fn::abc(age)` → `fn::abc`,
                // `RETURN name.len()` → `name`), verified on SurrealDB 3.0.5.
                match alias_name {
                    Some(alias) => {
                        fields.insert(alias, kind);
                    }
                    None => {
                        let path = match &expr.node {
                            ast::Expr::Idiom(idiom) => {
                                crate::analyzer::data::select::simplified_key_segments(idiom)
                            }
                            _ => None,
                        };
                        match path {
                            Some(segments) => {
                                crate::analyzer::data::select::insert_kind_at_path(
                                    &mut fields,
                                    &segments,
                                    kind,
                                );
                            }
                            None => {
                                fields.insert(
                                    crate::analyzer::data::select::unaliased_computed_key(
                                        expr,
                                        ctx.source_text(),
                                    ),
                                    kind,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    if fields.is_empty() {
        return Kind::Any;
    }
    Kind::Literal(KindLiteral::Object(fields))
}

/// The table a mutation target names (`person` / `person:one`).
pub fn source_table_name(source: Option<&ast::Spanned<ast::Expr>>) -> Option<String> {
    match source.map(|s| &s.node)? {
        ast::Expr::Table(name) => Some(name.node.clone()),
        ast::Expr::RecordId { table, .. } => Some(table.node.clone()),
        _ => None,
    }
}

fn slice(text: &str, range: ByteRange) -> &str {
    text[range.start() as usize..range.end() as usize].trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    use crate::schema::extract_schema;

    const PERSON_SCHEMA: &str = "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD age ON person TYPE int;";

    fn build_kind(schema_src: &str, query: &str, statement_kind: &str) -> Kind {
        let schema_parsed =
            parse_source(SourceId::new("schema"), schema_src).expect("schema should parse");
        let schema = extract_schema(&[schema_parsed]).schema;
        let parsed = parse_source(SourceId::new("query"), query).expect("query should parse");
        let table = schema.tables.get("person").expect("person table indexed");
        let env = crate::statement_env::StatementEnv::default();

        let lowered = surrealguard_syntax::lower::lower_first_statement(&parsed, statement_kind)
            .unwrap_or_else(|| panic!("{statement_kind} node exists in {query:?}"));
        let (only, ret) = match &lowered.node {
            ast::Statement::Create(s) => (s.only, s.ret.clone()),
            ast::Statement::Update(s) => (s.only, s.ret.clone()),
            other => panic!("unexpected statement {other:?}"),
        };
        let mut diagnostics: Vec<surrealguard_diagnostics::Finding> = Vec::new();
        let mut ctx = AnalysisContext::scoped(
            &schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
            env,
            None,
        );
        response_kind_for_target(only, ret.as_ref(), table, &mut ctx)
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
    fn return_projections_follow_the_engines_projection_naming() {
        // `UPDATE … RETURN string::len(name), name.len(), name.len() + 1`
        // returns `{string::len, name, "name.len() + 1"}` on SurrealDB 3.0.5:
        // a mutation's RETURN projections are named exactly like a SELECT's.
        let kind = build_kind(
            PERSON_SCHEMA,
            "UPDATE person SET age = 1 RETURN string::len(name), name.len(), name.len() + 1;",
            "UpdateStatement",
        );

        let fields = object_fields(array_element(&kind));
        assert_eq!(fields["string::len"], Kind::Int);
        assert!(fields.contains_key("name"), "got: {fields:?}");
        assert!(fields.contains_key("name.len() + 1"), "got: {fields:?}");
        assert!(!fields.contains_key("string::len(name)"), "got: {fields:?}");
        assert!(!fields.contains_key("name.len()"), "got: {fields:?}");
    }

    #[test]
    fn default_return_mode_infers_array_of_full_table_rows() {
        let kind = build_kind(PERSON_SCHEMA, "CREATE person;", "CreateStatement");

        let fields = object_fields(array_element(&kind));
        assert_eq!(fields["name"], Kind::String);
        assert_eq!(fields["age"], Kind::Int);
    }

    #[test]
    fn only_modifier_skips_the_array_wrapper() {
        let kind = build_kind(PERSON_SCHEMA, "CREATE ONLY person:one;", "CreateStatement");

        let fields = object_fields(&kind);
        assert_eq!(fields["name"], Kind::String);
    }

    #[test]
    fn return_none_infers_an_empty_array() {
        let kind = build_kind(
            PERSON_SCHEMA,
            "CREATE person RETURN NONE;",
            "CreateStatement",
        );

        assert_eq!(kind, Kind::Array(Box::new(Kind::Any), Some(0)));
    }

    #[test]
    fn only_with_return_none_is_the_none_kind() {
        let kind = build_kind(
            PERSON_SCHEMA,
            "CREATE ONLY person:one RETURN NONE;",
            "CreateStatement",
        );

        assert_eq!(kind, Kind::None);
    }

    #[test]
    fn return_diff_infers_the_fixed_patch_operation_kind() {
        let kind = build_kind(
            PERSON_SCHEMA,
            "UPDATE person RETURN DIFF;",
            "UpdateStatement",
        );

        let outer = array_element(&kind);
        let fields = object_fields(array_element(outer));
        assert_eq!(fields["op"], Kind::String);
        assert_eq!(fields["path"], Kind::String);
        assert_eq!(fields["value"], Kind::Any);
    }

    #[test]
    fn return_explicit_fields_infers_only_the_selected_fields_with_alias() {
        let kind = build_kind(
            PERSON_SCHEMA,
            "UPDATE person SET age = 30 RETURN age AS new_age;",
            "UpdateStatement",
        );

        let fields = object_fields(array_element(&kind));
        assert_eq!(fields.len(), 1);
        assert_eq!(fields["new_age"], Kind::Int);
    }

    #[test]
    fn return_of_a_keyword_like_field_name_is_a_fields_return_not_none() {
        // `nonexistent_field` contains "none" case-insensitively; it must
        // classify as an (unresolvable) fields return, never as an empty
        // RETURN NONE array.
        let kind = build_kind(
            PERSON_SCHEMA,
            "UPDATE person RETURN nonexistent_field;",
            "UpdateStatement",
        );

        let fields = object_fields(array_element(&kind));
        assert_eq!(fields["nonexistent_field"], Kind::Any);
    }

    // --- TG-1: implicit `id` / `in` / `out` on returned rows ---------------

    fn record_of(table: &str) -> Kind {
        Kind::Record(vec![surrealdb_types::Table::from(table)])
    }

    /// The returned row object of a mutation (unwrapping the array).
    fn row_fields(schema_src: &str, query: &str, statement_kind: &str) -> BTreeMap<String, Kind> {
        object_fields(array_element(&build_kind(schema_src, query, statement_kind))).clone()
    }

    #[test]
    fn every_full_row_return_mode_carries_the_implicit_id() {
        // Default, RETURN AFTER, RETURN BEFORE and RETURN * all yield full
        // materialized rows — every one of them has an `id`.
        for (query, statement_kind) in [
            ("CREATE person;", "CreateStatement"),
            ("CREATE person RETURN AFTER;", "CreateStatement"),
            ("UPDATE person SET age = 1 RETURN AFTER;", "UpdateStatement"),
            ("UPDATE person SET age = 1 RETURN BEFORE;", "UpdateStatement"),
            ("UPDATE person SET age = 1 RETURN *;", "UpdateStatement"),
        ] {
            let fields = row_fields(PERSON_SCHEMA, query, statement_kind);
            assert_eq!(fields["id"], record_of("person"), "{query}");
            assert_eq!(fields["name"], Kind::String, "{query}");
            assert_eq!(fields.len(), 3, "{query}: {fields:?}");
        }
    }

    #[test]
    fn only_mutation_rows_carry_the_implicit_id_too() {
        let kind = build_kind(PERSON_SCHEMA, "CREATE ONLY person:one;", "CreateStatement");

        let fields = object_fields(&kind);
        assert_eq!(fields["id"], record_of("person"));
    }

    #[test]
    fn a_relation_edge_row_carries_id_in_and_out() {
        const EDGE_SCHEMA: &str = "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD name ON person TYPE string;\n\
             DEFINE TABLE post SCHEMAFULL;\n\
             DEFINE FIELD title ON post TYPE string;\n\
             DEFINE TABLE likes SCHEMAFULL TYPE RELATION FROM person TO post;\n\
             DEFINE FIELD since ON likes TYPE datetime;";

        // `build_kind` resolves the `person` table; go through the shared
        // builder directly so the edge table is the target.
        let parsed = parse_source(SourceId::new("schema"), EDGE_SCHEMA).expect("schema parses");
        let schema = extract_schema(&[parsed]).schema;
        let table = schema.tables.get("likes").expect("likes indexed");
        let query = parse_source(SourceId::new("q"), "RELATE person:a->likes->post:b;")
            .expect("query parses");
        let mut diagnostics: Vec<surrealguard_diagnostics::Finding> = Vec::new();
        let mut ctx = AnalysisContext::scoped(
            &schema,
            query.source_id().clone(),
            query.text(),
            &mut diagnostics,
            crate::statement_env::StatementEnv::default(),
            None,
        );
        let kind = response_kind_for_target(false, None, table, &mut ctx);

        let fields = object_fields(array_element(&kind));
        assert_eq!(fields["id"], record_of("likes"));
        assert_eq!(fields["in"], record_of("person"));
        assert_eq!(fields["out"], record_of("post"));
        assert_eq!(fields["since"], Kind::Datetime);
    }

    #[test]
    fn return_modes_that_produce_no_row_gain_no_id() {
        // RETURN NONE: an empty array, not a row.
        assert_eq!(
            build_kind(PERSON_SCHEMA, "CREATE person RETURN NONE;", "CreateStatement"),
            Kind::Array(Box::new(Kind::Any), Some(0))
        );
        // RETURN DIFF: a patch list, not a row.
        let diff = build_kind(PERSON_SCHEMA, "UPDATE person RETURN DIFF;", "UpdateStatement");
        let patch = object_fields(array_element(array_element(&diff)));
        assert!(!patch.contains_key("id"), "got: {patch:?}");
        assert_eq!(patch.len(), 3);
        // RETURN <fields> projects exactly what was asked for.
        let projected = row_fields(
            PERSON_SCHEMA,
            "UPDATE person SET age = 30 RETURN age;",
            "UpdateStatement",
        );
        assert_eq!(projected.len(), 1);
        assert!(!projected.contains_key("id"), "got: {projected:?}");
    }
}
