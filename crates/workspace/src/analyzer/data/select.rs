//! `SELECT` statement analysis: response-type inference over the typed AST.
//!
//! The inferred response type is a plain upstream `Kind`: closed objects
//! are `Kind::Literal(KindLiteral::Object(...))`, undeterminable positions
//! are `Kind::Any` poison values. Graph traversals, destructure selections,
//! and modifier clauses are consumed as structured `ast::*` values. The
//! only use of source text is *naming*: an unaliased computed projection is
//! keyed by its own source text (`SELECT age >= 18 FROM ...` produces the
//! field `"age >= 18"`).
//!
//! The section at the bottom holds `SelectIr`-typed resolvers that exist
//! only for the remaining validators in `crate::semantic`.

use std::collections::BTreeMap;

use surrealdb_types::{Kind, KindLiteral};
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::expression::infer::{infer_expression_fact, plain_field_segments};
use crate::schema::{SchemaIndex, TableDef};
use crate::select_ir::{GraphDirection, GraphLookup, SelectIr};

pub fn analyze_select(ctx: &mut AnalysisContext<'_>, stmt: &ast::SelectStmt) -> Kind {
    select_response_kind(stmt, ctx)
}

/// Pure core: infers the response type of a lowered `SELECT`.
pub(crate) fn select_response_kind(stmt: &ast::SelectStmt, ctx: &mut AnalysisContext<'_>) -> Kind {
    if stmt.explain.is_some() {
        return explain_response_kind();
    }
    let Some(from) = stmt.from.first() else {
        return Kind::Any;
    };
    let table_name = match &from.node {
        ast::Expr::Table(name) => name.node.clone(),
        ast::Expr::RecordId { table, .. } => table.node.clone(),
        ast::Expr::Idiom(idiom) => match graph_source_table(idiom, ctx.schema()) {
            Some(table) => table,
            None => return Kind::Any,
        },
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
                return Kind::Any;
            }
            let inner_kind = ctx.with_row_table(None, |ctx| {
                crate::analyzer::expression::infer::statement_value_kind(inner, ctx)
            });
            let row_kind = match inner_kind {
                Some(Kind::Array(element, _)) => *element,
                Some(other) => other,
                None => return Kind::Any,
            };
            return if stmt.only {
                row_kind
            } else {
                Kind::Array(Box::new(row_kind), literal_limit(stmt))
            };
        }
        // Dynamic sources (params) and anything else stay undetermined.
        _ => return walk_projections_for_findings(stmt, ctx),
    };
    let Some(table) = ctx.schema().tables.get(&table_name) else {
        crate::analyzer::data::check_table_reference(ctx, &table_name, from.span);
        return walk_projections_for_findings(stmt, ctx);
    };
    if table.fields.is_empty() && !stmt.projections.iter().any(is_graph_projection) {
        return walk_projections_for_findings(stmt, ctx);
    }

    if let Some(cond) = &stmt.where_clause {
        // The WHERE kind is irrelevant to the response; the walk emits
        // findings inside the condition, with row fields resolvable.
        ctx.with_row_table(Some(table), |ctx| {
            infer_expression_fact(cond, ctx);
            crate::analyzer::expression::check::check_value_expression(ctx, cond);
        });
        crate::analyzer::data::check_expression_field_paths(ctx, table, cond, 1003);
    }

    // Row-context clauses reference fields by name; each position has its
    // own code so hosts can configure them independently.
    for idiom in &stmt.omit {
        if let Some(segments) = plain_field_segments(&idiom.node) {
            crate::analyzer::data::check_field_path(ctx, table, &segments, idiom.span, 1007);
        }
    }
    for idiom in &stmt.fetch {
        // FETCH also accepts projection aliases; only bare field names are
        // checkable here.
        let named_alias = stmt.projections.iter().any(|projection| {
            matches!(projection, ast::Projection::Expr { alias: Some(alias), .. }
                if idiom.node.parts.len() == 1
                    && matches!(&idiom.node.parts[0].node, ast::IdiomPart::Field(name) if *name == alias.node))
        });
        if named_alias {
            continue;
        }
        if let Some(segments) = plain_field_segments(&idiom.node) {
            crate::analyzer::data::check_field_path(ctx, table, &segments, idiom.span, 1008);
        }
    }
    for idiom in &stmt.split {
        if let Some(segments) = plain_field_segments(&idiom.node) {
            crate::analyzer::data::check_field_path(ctx, table, &segments, idiom.span, 1009);
        }
    }
    if let Some(group) = &stmt.group {
        for idiom in &group.keys {
            if let Some(segments) = plain_field_segments(&idiom.node) {
                crate::analyzer::data::check_field_path(ctx, table, &segments, idiom.span, 1010);
            }
        }
    }
    if let Some(order) = &stmt.order {
        for key in &order.keys {
            crate::analyzer::data::check_expression_field_paths(ctx, table, &key.expr, 1010);
        }
    }

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
    let row_kind = apply_omit(row_kind, &stmt.omit);
    let row_kind = apply_fetch(row_kind, &stmt.fetch, ctx.schema());
    let row_kind = apply_split(row_kind, &stmt.split);

    if stmt.only {
        row_kind
    } else {
        Kind::Array(Box::new(row_kind), literal_limit(stmt))
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

fn object_literal(fields: BTreeMap<String, Kind>) -> Kind {
    Kind::Literal(KindLiteral::Object(fields))
}

/// Resolves `FROM person->likes->post` to the traversal's target table.
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

/// The relation-endpoint check shared by the AST path and the legacy
/// `SelectIr` resolvers: does `edge`'s relation connect `source_table` to
/// `target` in direction `dir`?
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
            graph_projection_kind(row_table_name, idiom, ctx.schema(), false)
        }
        ast::Expr::Idiom(idiom) => {
            let segments = plain_field_segments(idiom)?;
            kind_for_path(table, &segments)
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
            // `profile.{email, city}` selects sub-fields of a row field.
            if let Some(outputs) = destructure_kinds(table, &prefix, &selected) {
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
            if let Some(kind) = kind_for_path(table, &segments) {
                match &alias_name {
                    Some(alias) => {
                        fields.insert(alias.clone(), kind);
                    }
                    None => insert_kind_at_path(fields, &segments, kind),
                }
                return;
            }
            // Known-plain path that doesn't resolve: poison entry + finding.
            crate::analyzer::data::check_field_path(ctx, table, &segments, expr.span, 1002);
            fields.insert(alias_name.unwrap_or_else(|| segments.join(".")), Kind::Any);
            return;
        }

        // Idioms with parts we don't project yet (Start/Index/Method/...).
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
    // Re-resolve the table from the schema so the borrow carries the
    // context's lifetime rather than the caller's.
    let table = ctx.schema().tables.get(&table.name);
    ctx.with_row_table(table, |ctx| {
        crate::analyzer::expression::check::check_value_expression(ctx, expr);
        infer_expression_fact(expr, ctx).kind.unwrap_or(Kind::Any)
    })
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
                    object.insert(name, kind_for_path(target, &segments)?);
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
        let selected_kind = kind_for_path(target, &segments)?;
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
    table: &TableDef,
    prefix: &[String],
    selected: &[ast::Spanned<ast::Idiom>],
) -> Option<Vec<(Vec<String>, Kind)>> {
    let mut outputs = Vec::new();
    for sub in selected {
        let sub_segments = plain_field_segments(&sub.node)?;
        let mut segments = prefix.to_vec();
        segments.extend(sub_segments);
        let kind = kind_for_path(table, &segments)?;
        outputs.push((segments, kind));
    }
    Some(outputs)
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

    table
        .fields
        .get(&segments.join("."))
        .map(|field| field.kind.clone().unwrap_or(Kind::Any))
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

// ---------------------------------------------------------------------------
// SelectIr-typed resolvers, used only by the validators in
// `crate::semantic`; they delegate to the shared relation core above.
// ---------------------------------------------------------------------------

pub(crate) fn resolved_select_table_name(ir: &SelectIr, schema: &SchemaIndex) -> Option<String> {
    let source_table = ir.source.as_ref()?.table.as_ref()?;
    if ir.graph_lookups.is_empty() {
        return Some(source_table.clone());
    }

    if !ir.graph_lookups.len().is_multiple_of(2) {
        return None;
    }
    let mut current = source_table.clone();
    for pair in ir.graph_lookups.chunks_exact(2) {
        current = resolve_graph_step_target_table(&current, &pair[0], &pair[1], schema)?;
    }
    Some(current)
}

pub(crate) fn resolve_graph_step_target_table(
    source_table: &str,
    edge_lookup: &GraphLookup,
    target_lookup: &GraphLookup,
    schema: &SchemaIndex,
) -> Option<String> {
    let edge = edge_lookup.table.as_deref()?;
    let target = target_lookup.table.as_deref()?;
    let dir = match edge_lookup.direction {
        GraphDirection::Out => ast::GraphDir::Out,
        GraphDirection::In => ast::GraphDir::In,
        GraphDirection::Both => ast::GraphDir::Both,
    };
    relation_step_target(source_table, dir, edge, target, schema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::SchemaIndex;
    use crate::statement_env::StatementEnv;
    use surrealguard_syntax::lower::lower_statement;
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
        let node = crate::analyzer::test_support::find_first_node(
            parsed.tree().root_node(),
            "SelectStatement",
        )
        .expect("select statement exists");
        match lower_statement(node, parsed.text()).node {
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
}
