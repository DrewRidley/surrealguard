//! Graph-traversal invariant checking.
//!
//! Walks a graph idiom step by step against the schema's relations,
//! emitting findings where a traversal cannot mean what it says: edges
//! that aren't relations (3001), relations that don't connect the source
//! in the written direction (3002), unreachable target hops (3003),
//! unresolvable multi-target steps (3005). Step-local `WHERE` filters —
//! both the inline `->(likes WHERE ...)` form and the bracketed
//! `->likes[WHERE ...]` form — are checked against the table they filter
//! (the edge, or the node table once a hop lands): unknown fields are
//! 1003, operator misuse inside them is the usual expression checking.
//!
//! Parentheses around a target are not a wall: `->(likes)` is `->likes`,
//! and everything the parenthesized form can add — a filter, several
//! targets, `?`, a record range, a `SELECT … FROM` projection — resolves
//! and is checked through the same code the bare form goes through. A step
//! that names no single table stops the traversal's type, and says which
//! of those it was (6003) rather than leaving the site a silent `any`.
//!
//! A field read is not a wall either. The walk carries the segments it passes
//! and resolves them the moment another `->` arrives, so a traversal that
//! resumes after a tail (`->wrote->post.author->follows->user`) steps from what
//! the field NAMES rather than from where the last hop landed. The match over
//! parts is exhaustive on purpose: what each one does to the table the next
//! step traverses from is a decision, and a wrong answer here is a false
//! finding on a valid query.
//!
//! The kind of a traversal comes from [`super::select`]'s resolvers; this
//! module is the checking-side twin, invoked from the same sites. Everything
//! BEHIND a step — the fields, indexes, splats and methods of its tail — is
//! checked by the ordinary idiom walk
//! ([`crate::analyzer::expression::check`]), not here; `FROM` is the one
//! position that walk never sees, so this module checks that one's fields too.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;
use surrealql_analyzer_syntax::span::{ByteRange, SourceSpan};

use crate::analyzer::context::AnalysisContext;
use crate::schema::TableDef;

/// Checks every graph step of `idiom`, starting from `source_table` (the
/// row context of the enclosing statement, or a leading field's record
/// target). Non-graph leading parts are skipped; checking begins at the
/// first graph part.
pub(crate) fn check_graph_idiom(
    ctx: &mut AnalysisContext<'_>,
    source_table: &str,
    idiom: &ast::Idiom,
) {
    check_graph_idiom_at(ctx, source_table, idiom, false);
}

/// `require_landing` is FROM's extra contract: the traversal must end on
/// a table, not in the middle of a hop (3004).
pub(crate) fn check_graph_idiom_at(
    ctx: &mut AnalysisContext<'_>,
    source_table: &str,
    idiom: &ast::Idiom,
    require_landing: bool,
) {
    // `current` is the table the traversal stands on before each part;
    // `None` after a step that failed to resolve (stop checking — one
    // finding per broken chain, not a cascade).
    let mut current: Option<String> = Some(source_table.to_string());
    // The table a bracketed `[WHERE ...]` immediately after a part filters:
    // the edge table right after `->likes`, the node table after a hop.
    let mut filter_table: Option<String> = Some(source_table.to_string());
    // The edge of the previous step, for verifying the landing half of a
    // hop pair (`->likes->comment` when `likes` only reaches `post`).
    let mut pending_edge: Option<(String, ast::GraphDir)> = None;
    // Field segments read since the last graph step. A traversal must start
    // from records, so they are resolved — once — the moment another `->`
    // arrives: `author->follows->user` and `->wrote->post.author->follows->user`
    // are the SAME question asked in two places, and only the leading spelling
    // used to be asked at all.
    let mut pending_fields: Vec<(String, ByteRange)> = Vec::new();

    // In a `FROM` target the leading name is the SOURCE TABLE, not a field of
    // one (`FROM user->follows->user`): the caller already resolved it into
    // `source_table`, so walking it again as a field would look for a `user`
    // field on `user`. Every other position's leading name really is a field.
    let parts = match (require_landing, idiom.parts.first().map(|part| &part.node)) {
        (true, Some(ast::IdiomPart::Field(_))) => &idiom.parts[1..],
        _ => &idiom.parts[..],
    };

    for part in parts {
        match &part.node {
            ast::IdiomPart::Graph { dir, step } => {
                if !pending_fields.is_empty() {
                    check_source_fields(ctx, require_landing, current.as_deref(), &pending_fields);
                    current =
                        rebase_through_fields(ctx, current.as_deref(), &pending_fields, part.span);
                    pending_edge = None;
                    pending_fields.clear();
                }
                let Some(source) = current.clone() else {
                    return;
                };
                report_unresolved_step(ctx, part.span, step);
                let step_result = check_step(ctx, &source, dir.node, step, pending_edge.is_some());

                // A plain-table landing completes a hop: verify every name
                // it lists is on the previous edge's far side. `->(a, b)`
                // lists more than one; each is a landing in its own right.
                if step_result.edge_table.is_none() {
                    if let Some((edge, edge_dir)) = &pending_edge {
                        for target in &step.targets {
                            check_hop_reachability(ctx, edge, *edge_dir, target);
                        }
                    }
                }

                filter_table = step_result
                    .edge_table
                    .clone()
                    .or_else(|| step_result.landed_on.clone());

                // An inline `->(X WHERE ...)` filters the rows the step
                // produced — the edge for `->(likes WHERE ...)`, the node
                // for the landing half of a hop, `->likes->(post WHERE
                // ...)`. That is exactly the table a bracketed
                // `->likes[WHERE ...]` filters, so both forms read the same
                // `filter_table`; keying the inline form off the edge alone
                // silently dropped every landing-step filter.
                if let (Some(cond), Some(table)) = (&step.where_clause, filter_table.clone()) {
                    check_filter(ctx, &table, cond);
                }

                // A step's own `LIMIT`/`START` is the statement's clause
                // written somewhere else; the contract does not move with it.
                for (clause, name, position) in [
                    (
                        &step.limit,
                        "LIMIT",
                        crate::analyzer::contract::Position::Limit,
                    ),
                    (
                        &step.start,
                        "START",
                        crate::analyzer::contract::Position::Start,
                    ),
                ] {
                    if let Some(expr) = clause {
                        crate::analyzer::data::select::check_row_count_clause(
                            ctx, expr, name, position,
                        );
                    }
                }

                pending_edge = if step_result.violated {
                    None
                } else {
                    step_result.edge_table.map(|edge| (edge, dir.node))
                };
                if let Some(landed) = step_result.landed_on {
                    current = Some(landed);
                }
            }
            ast::IdiomPart::Where(cond) => {
                if let Some(table) = filter_table.clone() {
                    check_filter(ctx, &table, cond);
                }
            }
            ast::IdiomPart::Recurse { bounded } => {
                if !bounded {
                    ctx.emit(
                        surrealql_analyzer_diagnostics::catalog::finding(
                            SourceSpan::new(ctx.source().clone(), part.span),
                            3011,
                            "this recursion has no upper bound and can walk the entire graph"
                                .to_string(),
                        )
                        .with_help("give the range an upper bound, e.g. `{1..5}`"),
                    );
                }
                current = None;
                filter_table = None;
                pending_edge = None;
                pending_fields.clear();
            }
            // Every other part is here because it answers one question: what
            // does the NEXT graph step traverse from? An exhaustive match is
            // the whole point — a new `IdiomPart` cannot be added without
            // someone deciding, and a wrong answer here is a false 3001/3002 on
            // a valid query rather than a missing one.
            //
            // A field read moves the walk onto whatever it names (resolved when
            // the next step arrives); `.*`, an index, `[$]`, `?` and `...` all
            // leave it standing on the same rows; a `.{…}`, a method and an
            // unlowered part replace the value with something no traversal can
            // continue from.
            ast::IdiomPart::Field(name) => pending_fields.push((name.clone(), part.span)),
            ast::IdiomPart::All
            | ast::IdiomPart::Index(_)
            | ast::IdiomPart::Last
            | ast::IdiomPart::Optional
            | ast::IdiomPart::Flatten => {}
            ast::IdiomPart::Destructure(_)
            | ast::IdiomPart::Method { .. }
            | ast::IdiomPart::Partial(_)
            | ast::IdiomPart::Start(_) => {
                current = None;
                filter_table = None;
                pending_edge = None;
                pending_fields.clear();
            }
        }
    }

    // A field tail with no step behind it. `FROM user->follows->user.john`
    // reads a field like any other position does — and this is the only
    // position where saying so is this function's job.
    check_source_fields(ctx, require_landing, current.as_deref(), &pending_fields);

    if require_landing {
        if let Some((edge, _)) = &pending_edge {
            if let Some(last) = idiom.parts.last() {
                ctx.emit(
                    surrealql_analyzer_diagnostics::catalog::finding(
                        SourceSpan::new(ctx.source().clone(), last.span),
                        3004,
                        format!("this FROM target stops on the edge `{edge}`, not on a table"),
                    )
                    .with_help(format!("add a landing step, e.g. `->{edge}->target`")),
                );
            }
        }
    }
}

/// A step that names no single table ends the traversal's type — `any` from
/// here on. Say which of the three reasons it was, at the step, rather than
/// leaving the projection a silent `any`: `?` names every edge, `->(a, b)`
/// names several, and a target the grammar admits but SurrealDB does not
/// (`->(post.{title})`) names none.
///
/// 6003 is the "analyzer could not resolve this" code, hint-severity and
/// allow-by-default: the query may well be right, we just cannot type it.
fn report_unresolved_step(ctx: &mut AnalysisContext<'_>, span: ByteRange, step: &ast::GraphStep) {
    for unmodeled in &step.unmodeled {
        // `Fields` is the step's own `SELECT … FROM` projection; anything
        // else stood where the table name goes.
        let (message, help) = if unmodeled.cst_kind == "Fields" {
            (
                "surrealql-analyzer can't type what this graph selection projects",
                "plain field names, `*`, and `VALUE <path>` are modeled here; \
                 an alias or a dotted key is not"
                    .to_string(),
            )
        } else {
            (
                "surrealql-analyzer can't resolve this graph target",
                format!(
                    "SurrealDB accepts a table name, `?`, or a record range after `->`/`<-`, \
                     not a `{}`",
                    unmodeled.cst_kind
                ),
            )
        };
        ctx.emit(
            surrealql_analyzer_diagnostics::catalog::finding(
                SourceSpan::new(ctx.source().clone(), unmodeled.span),
                6003,
                message.to_string(),
            )
            .with_help(help),
        );
    }
    if step.wildcard {
        ctx.emit(
            surrealql_analyzer_diagnostics::catalog::finding(
                SourceSpan::new(ctx.source().clone(), span),
                6003,
                "`?` traverses every edge, so surrealql-analyzer can't name what this step reaches"
                    .to_string(),
            )
            .with_help("name the edge or table to have the traversal typed"),
        );
        return;
    }
    if step.targets.len() > 1 {
        ctx.emit(
            surrealql_analyzer_diagnostics::catalog::finding(
                SourceSpan::new(ctx.source().clone(), span),
                6003,
                format!(
                    "this step names {} tables, so surrealql-analyzer can't resolve the traversal to one",
                    step.targets.len()
                ),
            )
            .with_help("each name is still checked; only the resulting type is left open"),
        );
    }
}

fn field_names(fields: &[(String, ByteRange)]) -> Vec<String> {
    fields.iter().map(|(name, _)| name.clone()).collect()
}

/// The field segments of a `FROM` traversal, checked against the table they are
/// read off (1002).
///
/// `FROM` is the one position whose idiom is *not* walked by
/// [`crate::analyzer::expression::check::check_idiom_positions`] — it resolves
/// its source table here instead — so a field named in it is checked here or
/// nowhere. `FROM user->follows->user.john` is a legal, degenerate target: on
/// 3.2.3 it yields `[]` where `.name` yields `['Bob']`, exactly the difference
/// a missing field makes anywhere else.
///
/// Every other position leaves this to the idiom walk, which checks each
/// segment against the value in front of it — so this is guarded rather than
/// unconditional, and the guard is the position, not a heuristic.
fn check_source_fields(
    ctx: &mut AnalysisContext<'_>,
    require_landing: bool,
    current: Option<&str>,
    fields: &[(String, ByteRange)],
) {
    if !require_landing || fields.is_empty() {
        return;
    }
    let Some(table) = current.and_then(|name| ctx.schema().tables.get(name)) else {
        return;
    };
    let span = match (fields.first(), fields.last()) {
        (Some((_, first)), Some((_, last))) => ByteRange::new(first.start(), last.end()).ok(),
        _ => None,
    };
    let Some(span) = span else {
        return;
    };
    crate::analyzer::data::select::validate_field_path(
        ctx,
        table,
        &field_names(fields),
        span,
        1002,
    );
}

/// The table a graph step traverses from, after a run of field reads.
///
/// A traversal must start from records. A path that lands on a single record
/// link rebases the walk onto that link's table (`post.author->follows->user`
/// steps from `user`); one that lands on a value which provably holds no
/// records cannot step at all (3009); and one that cannot be resolved at all
/// hands back `None`, which stops the checking of this chain rather than
/// letting it continue against a table it has already left. That last case is
/// where the false `3001`/`3002` on a perfectly valid
/// `->wrote->post.author->follows->user` came from: the walk kept `post` and
/// asked whether `follows` steps off it.
fn rebase_through_fields(
    ctx: &mut AnalysisContext<'_>,
    current: Option<&str>,
    fields: &[(String, ByteRange)],
    graph_span: ByteRange,
) -> Option<String> {
    let table = ctx.schema().tables.get(current?)?;
    let fields = &field_names(fields);
    let kind = crate::analyzer::data::select::resolve_field_path(ctx.schema(), table, fields)?;
    if kind != Kind::Any && !kind_is_recordish(&kind) {
        ctx.emit(
            surrealql_analyzer_diagnostics::catalog::finding(
                SourceSpan::new(ctx.source().clone(), graph_span),
                3009,
                format!(
                    "a graph step can't start from `{}` — `{}` holds no records",
                    fields.join("."),
                    crate::render_kind(&kind)
                ),
            )
            .with_help("`->`/`<-` traverse from records; this field is not a record link"),
        );
        return None;
    }
    single_record_target(&kind)
}

/// The single table a record-ish kind links to, when unambiguous.
fn single_record_target(kind: &Kind) -> Option<String> {
    match kind {
        Kind::Record(targets) => match targets.as_slice() {
            [only] => Some(only.to_string()),
            _ => None,
        },
        Kind::Array(element, _) | Kind::Set(element, _) => single_record_target(element),
        _ => None,
    }
}

fn kind_is_recordish(kind: &Kind) -> bool {
    match kind {
        Kind::Record(_) | Kind::Any => true,
        Kind::Array(element, _) | Kind::Set(element, _) => kind_is_recordish(element),
        Kind::Either(variants) => variants.iter().any(kind_is_recordish),
        _ => false,
    }
}

struct StepOutcome {
    /// The edge table when the step named exactly one known relation.
    edge_table: Option<String>,
    /// The table the traversal stands on after this step: relations keep
    /// the walk going even when the far side is ambiguous — `None` only
    /// when checking cannot meaningfully continue.
    landed_on: Option<String>,
    /// The step already violated the shape contract; downstream checks on
    /// the same chain stay quiet (one finding per broken traversal).
    violated: bool,
}

fn check_step(
    ctx: &mut AnalysisContext<'_>,
    source: &str,
    dir: ast::GraphDir,
    step: &ast::GraphStep,
    after_edge: bool,
) -> StepOutcome {
    let [target] = step.targets.as_slice() else {
        // `->(a, b)` is valid — but what each name must *be* depends on
        // where the step sits. A traversal step names relations; the
        // landing half of a hop pair names node tables. Checking every
        // multi-target step as relations reported the valid landing
        // `->wrote->(post, comment)` as two 3001s. Not resolving the
        // landing to one table is an analyzer limitation, not a contract
        // violation.
        for target in &step.targets {
            if after_edge {
                crate::analyzer::data::check_table_reference(ctx, &target.node, target.span);
            } else {
                check_edge_is_relation(ctx, &target.node, target.span);
            }
        }
        return StepOutcome {
            edge_table: None,
            landed_on: None,
            violated: false,
        };
    };
    let edge = target.node.as_str();

    // The step names either an edge (`->likes`) or, on the second half of
    // a hop pair, a node table (`->post`). Distinguish by what the schema
    // says: a relation table is an edge; a plain table is a landing.
    let relation = ctx
        .schema()
        .tables
        .get(edge)
        .and_then(|table| table.relation.clone());

    let Some(relation) = relation else {
        if ctx.schema().tables.contains_key(edge) {
            // A plain table is a valid landing only right after an edge
            // (`->likes->comment`). With nothing to land from, the step
            // is a traversal — and a traversal must name a relation.
            if !after_edge {
                ctx.emit(
                    surrealql_analyzer_diagnostics::catalog::finding(
                        SourceSpan::new(ctx.source().clone(), target.span),
                        3001,
                        format!("`{edge}` can't be traversed — it is not a relation table"),
                    )
                    .with_help(
                        "only tables defined with `TYPE RELATION` can be stepped through with `->`/`<-`",
                    ),
                );
                return StepOutcome {
                    edge_table: None,
                    landed_on: None,
                    violated: true,
                };
            }
            return StepOutcome {
                edge_table: None,
                landed_on: Some(edge.to_string()),
                violated: false,
            };
        }
        // Not in the schema at all: the standardized unknown-table finding.
        crate::analyzer::data::check_table_reference(ctx, edge, target.span);
        return StepOutcome {
            edge_table: None,
            landed_on: None,
            violated: false,
        };
    };

    // The edge exists and is a relation: does it accept `source` on the
    // near side of this direction?
    let accepts = match dir {
        ast::GraphDir::Out => relation.in_tables.iter().any(|t| t == source),
        ast::GraphDir::In => relation.out_tables.iter().any(|t| t == source),
        ast::GraphDir::Both => {
            relation.in_tables.iter().any(|t| t == source)
                || relation.out_tables.iter().any(|t| t == source)
        }
    };
    if !accepts {
        emit_with_declaration(
            ctx,
            target.span,
            3002,
            format!(
                "relation `{edge}` connects {}, but this step traverses {} from `{source}`",
                declared_shape(edge, &relation),
                arrow_text(dir),
            ),
            edge,
        );
        return StepOutcome {
            edge_table: Some(edge.to_string()),
            landed_on: None,
            violated: true,
        };
    }

    // The traversal now stands "on the edge"; the far side resolves when
    // it is unambiguous, so a following node step can be verified as
    // reachable (3003 fires there via the accepts check against the far
    // table set).
    let far = match dir {
        ast::GraphDir::Out => &relation.out_tables,
        ast::GraphDir::In => &relation.in_tables,
        ast::GraphDir::Both => {
            return StepOutcome {
                edge_table: Some(edge.to_string()),
                landed_on: None,
                violated: false,
            };
        }
    };
    StepOutcome {
        edge_table: Some(edge.to_string()),
        landed_on: match far.as_slice() {
            [only] => Some(only.to_string()),
            _ => None,
        },
        violated: false,
    }
}

/// The reachability half of a hop pair: `->likes->comment` when `likes`
/// only reaches `post`. Called where the *pair* is known — the select
/// resolvers walk pairs; here the check runs when an edge's far side is
/// singular and the next part landed elsewhere.
pub(crate) fn check_hop_reachability(
    ctx: &mut AnalysisContext<'_>,
    edge: &str,
    dir: ast::GraphDir,
    target: &ast::Spanned<String>,
) {
    let Some(relation) = ctx
        .schema()
        .tables
        .get(edge)
        .and_then(|table| table.relation.clone())
    else {
        return;
    };
    let far = match dir {
        ast::GraphDir::Out => &relation.out_tables,
        ast::GraphDir::In => &relation.in_tables,
        ast::GraphDir::Both => return,
    };
    if far.contains(&target.node) {
        return;
    }
    emit_with_declaration(
        ctx,
        target.span,
        3002,
        format!(
            "relation `{edge}` connects {}, so this hop cannot land on `{}`",
            declared_shape(edge, &relation),
            target.node,
        ),
        edge,
    );
}

fn check_edge_is_relation(ctx: &mut AnalysisContext<'_>, edge: &str, span: ByteRange) {
    match ctx.schema().tables.get(edge) {
        Some(table) if table.relation.is_none() => {
            ctx.emit(
                surrealql_analyzer_diagnostics::catalog::finding(
                    SourceSpan::new(ctx.source().clone(), span),
                    3001,
                    format!("`{edge}` can't be traversed — it is not a relation table"),
                )
                .with_help(
                    "only tables defined with `TYPE RELATION` can be stepped through with `->`/`<-`",
                ),
            );
        }
        Some(_) => {}
        None => {
            crate::analyzer::data::check_table_reference(ctx, edge, span);
        }
    }
}

/// A step-local WHERE runs with the step's table as its row: unknown
/// fields are 1003 against that table, and operator misuse is the usual
/// expression checking.
fn check_filter(ctx: &mut AnalysisContext<'_>, table_name: &str, cond: &ast::Spanned<ast::Expr>) {
    let Some(table) = ctx.schema().tables.get(table_name) else {
        return;
    };
    ctx.with_row_table(Some(table), |ctx| {
        crate::analyzer::expression::infer::infer_expression_fact(cond, ctx);
        crate::analyzer::expression::check::check_value_expression(ctx, cond);
    });
    check_filter_fields(ctx, table, cond);
}

fn check_filter_fields(
    ctx: &mut AnalysisContext<'_>,
    table: &TableDef,
    cond: &ast::Spanned<ast::Expr>,
) {
    crate::analyzer::data::check_expression_field_paths(ctx, table, cond, 1002);
}

fn arrow_text(dir: ast::GraphDir) -> &'static str {
    match dir {
        ast::GraphDir::Out => "`->`",
        ast::GraphDir::In => "`<-`",
        ast::GraphDir::Both => "`<->`",
    }
}

/// The relation's declared shape, reader-facing: `` `person`->`post` ``.
pub(crate) fn declared_shape(edge: &str, relation: &crate::schema::RelationDef) -> String {
    format!(
        "{}->`{edge}`->{}",
        table_list(&relation.in_tables),
        table_list(&relation.out_tables)
    )
}

pub(crate) fn table_list(tables: &[String]) -> String {
    tables
        .iter()
        .map(|t| format!("`{t}`"))
        .collect::<Vec<_>>()
        .join("|")
}

/// Emits `code` pointing back at the relation's `DEFINE TABLE` so the
/// declared shape and the violating usage read side by side.
fn emit_with_declaration(
    ctx: &mut AnalysisContext<'_>,
    span: ByteRange,
    code: u16,
    message: String,
    edge: &str,
) {
    let declared_at = ctx
        .schema()
        .tables
        .get(edge)
        .map(|table| table.name_span.clone());
    let span = SourceSpan::new(ctx.source().clone(), span);
    let mut finding = surrealql_analyzer_diagnostics::catalog::finding(span, code, message);
    if let Some(declared_at) = declared_at {
        finding = finding.with_related(declared_at, format!("relation `{edge}` declared here"));
    }
    ctx.emit(finding);
}

#[cfg(test)]
mod tests {
    /// The schema every case below traverses: `user ->wrote-> post`.
    const SCHEMA: &str = "\
DEFINE TABLE user SCHEMAFULL;
DEFINE FIELD name ON user TYPE string;
DEFINE TABLE post SCHEMAFULL;
DEFINE FIELD title ON post TYPE string;
DEFINE TABLE comment SCHEMAFULL;
DEFINE TABLE wrote TYPE RELATION IN user OUT post SCHEMAFULL;
DEFINE FIELD since ON wrote TYPE datetime;
";

    /// `CODE message` for every finding a query raises against `SCHEMA`,
    /// minus the allow-by-default lints that say nothing about graphs.
    fn findings(query: &str) -> Vec<String> {
        let mut workspace = crate::Workspace::default();
        workspace.add_virtual_source("schema".into(), SCHEMA.into());
        let source = workspace.add_virtual_source("query".into(), query.into());
        let output = crate::analyze_workspace(&workspace);
        output.sources[&source]
            .diagnostics
            .iter()
            .filter(|finding| finding.code().number() != 7014)
            .map(|finding| format!("{} {}", finding.code(), finding.message()))
            .collect()
    }

    #[test]
    fn an_inline_filter_is_checked_on_the_rows_the_step_produced() {
        // The edge half filters the edge, the landing half filters the node.
        // Both are the *same* contract as the bracketed form, so both must
        // report the same way — `->wrote->(post WHERE …)` used to be dropped
        // entirely because the step resolved to a landing, not an edge.
        assert_eq!(
            findings("SELECT ->(wrote WHERE since != 5) FROM user;"),
            vec!["E2004 `!=` can't combine a `datetime` and a `int`"]
        );
        assert_eq!(
            findings("SELECT ->wrote->(post WHERE title != 5) FROM user;"),
            vec!["E2004 `!=` can't combine a `string` and a `int`"]
        );
        assert_eq!(
            findings("SELECT ->wrote->(post WHERE nope = 1) FROM user;"),
            vec!["E1002 `post` has no field `nope`"]
        );
        // The bracketed spelling of the same filter, for the same table.
        assert_eq!(
            findings("SELECT ->wrote->post[WHERE title != 5] FROM user;"),
            vec!["E2004 `!=` can't combine a `string` and a `int`"]
        );
    }

    #[test]
    fn a_filter_that_holds_up_is_silent() {
        assert!(findings("SELECT ->wrote->(post WHERE title = 'x') FROM user;").is_empty());
        assert!(findings("SELECT ->(wrote WHERE since > time::now())->post FROM user;").is_empty());
    }

    #[test]
    fn a_multi_target_step_is_read_by_position_not_as_edges_everywhere() {
        // After an edge, `->(a, b)` lists landings: `post` is on `wrote`'s
        // far side and is silent; `comment` is not, and that is 3002. Reading
        // both as edges reported two 3001s on a query the engine runs fine.
        assert_eq!(
            findings("SELECT ->wrote->(post, comment) FROM user;"),
            vec![
                "E6003 this step names 2 tables, so surrealql-analyzer can't resolve the traversal to one",
                "E3002 relation `wrote` connects `user`->`wrote`->`post`, so this hop cannot land on `comment`",
            ]
        );
        assert_eq!(
            findings("SELECT ->wrote->(post, ghost) FROM user;"),
            vec![
                "E6003 this step names 2 tables, so surrealql-analyzer can't resolve the traversal to one",
                "E1001 `ghost` is not a defined table",
                "E3002 relation `wrote` connects `user`->`wrote`->`post`, so this hop cannot land on `ghost`",
            ]
        );
        // In traversal position they are still edges, and must be relations.
        assert_eq!(
            findings("SELECT ->(wrote, post) FROM user;"),
            vec![
                "E6003 this step names 2 tables, so surrealql-analyzer can't resolve the traversal to one",
                "E3001 `post` can't be traversed — it is not a relation table",
            ]
        );
    }

    /// The kind a one-projection SELECT gives its single column.
    fn projected(query: &str) -> String {
        let mut workspace = crate::Workspace::default();
        workspace.add_virtual_source("schema".into(), SCHEMA.into());
        let source = workspace.add_virtual_source("query".into(), query.into());
        let output = crate::analyze_workspace(&workspace);
        let kind = output.sources[&source].statements[0]
            .response_kind
            .clone()
            .expect("a SELECT responds");
        crate::render_kind(&kind)
    }

    #[test]
    fn a_parenthesized_target_resolves_exactly_as_the_bare_one_does() {
        // Parentheses around a target are not a wall analysis stops at: the
        // step means the same thing with or without them, and a record range
        // over a table walks to that table's records.
        assert_eq!(
            projected("SELECT ->wrote->post AS p FROM user;"),
            "array<{ p: array<record<post>> }>"
        );
        assert_eq!(
            projected("SELECT ->wrote->(post) AS p FROM user;"),
            "array<{ p: array<record<post>> }>"
        );
        assert_eq!(
            projected("SELECT ->wrote->(post WHERE title = 'x') AS p FROM user;"),
            "array<{ p: array<record<post>> }>"
        );
        assert_eq!(
            projected("SELECT ->wrote->(post:1..9) AS p FROM user;"),
            "array<{ p: array<record<post>> }>"
        );
    }

    #[test]
    fn a_target_that_names_no_one_table_says_so_instead_of_going_quiet() {
        // Every one of these was `any` with nothing said about it. They stay
        // `any` — none of them names a table we could resolve — but each now
        // carries the reason.
        assert_eq!(
            findings("SELECT ->? AS p FROM user;"),
            vec![
                "E6003 `?` traverses every edge, so surrealql-analyzer can't name what this step reaches"
            ]
        );
        assert_eq!(
            findings("SELECT ->wrote->(?) AS p FROM user;"),
            vec![
                "E6003 `?` traverses every edge, so surrealql-analyzer can't name what this step reaches"
            ]
        );
        // Targets the vendored grammar admits and SurrealDB's parser rejects
        // (verified against 3.2.3): the destructure and splat belong *after*
        // the closing paren, and a single record id is not a target at all.
        for query in [
            "SELECT ->wrote->(post.{title}) AS p FROM user;",
            "SELECT ->wrote->(post.*) AS p FROM user;",
            "SELECT ->wrote->(post->wrote) AS p FROM user;",
            "SELECT ->wrote->(post:one) AS p FROM user;",
        ] {
            assert_eq!(
                findings(query),
                vec!["E6003 surrealql-analyzer can't resolve this graph target"],
                "for {query}"
            );
            assert_eq!(projected(query), "array<{ p: any }>", "for {query}");
        }
        // And the spelling SurrealDB *does* accept for the same intent is
        // resolved and checked, parentheses or not.
        assert_eq!(
            findings("SELECT ->wrote->(post).{nope} AS p FROM user;"),
            vec!["E1002 `post` has no field `nope`"]
        );
    }

    #[test]
    fn a_steps_own_limit_start_answers_to_the_same_contract_as_a_statements() {
        // The engine enforces it in both places — "LIMIT must be a
        // non-negative integer" on `->(post LIMIT -1)` as much as on a
        // statement's own clause (3.2.3) — so the step reads the same check
        // rather than treating the parentheses as a place the rule relaxes.
        assert_eq!(
            findings("SELECT ->wrote->(post LIMIT -1) FROM user;"),
            vec!["E2018 LIMIT can't be negative"]
        );
        assert_eq!(
            findings("SELECT ->wrote->(post LIMIT 3 START -2) FROM user;"),
            vec!["E2018 START can't be negative"]
        );
        assert!(findings("SELECT ->wrote->(post LIMIT 3 START 1) FROM user;").is_empty());
        // And it narrows how many rows come back, never their shape.
        assert_eq!(
            projected("SELECT ->wrote->(post LIMIT 3) AS p FROM user;"),
            "array<{ p: array<record<post>> }>"
        );
    }

    #[test]
    fn a_graph_selection_projects_rows_and_is_typed_as_what_it_projects() {
        // `->(SELECT a, b FROM t)` hands back projected objects, not links —
        // it used to come back as `array<record<post>>`, which is not a
        // shape the engine ever returns for it (3.2.3). Each spelling types
        // as its path equivalent: `SELECT a, b` is `.{a, b}`, `SELECT *` is
        // `.*`, and `SELECT VALUE a` is `.a`.
        assert_eq!(
            projected("SELECT ->wrote->(SELECT title FROM post) AS p FROM user;"),
            "array<{ p: array<{ title: string }> }>"
        );
        assert_eq!(
            projected("SELECT ->wrote->post.{title} AS p FROM user;"),
            "array<{ p: array<{ title: string }> }>"
        );
        assert_eq!(
            projected("SELECT ->wrote->(SELECT VALUE title FROM post) AS p FROM user;"),
            "array<{ p: array<string> }>"
        );
        assert_eq!(
            projected("SELECT ->wrote->(SELECT * FROM post) AS p FROM user;"),
            "array<{ p: array<{ id: record<post>, title: string }> }>"
        );
        // The projected fields are checked against the table they come off.
        assert_eq!(
            findings("SELECT ->wrote->(SELECT nope FROM post) AS p FROM user;"),
            vec!["E1002 `post` has no field `nope`"]
        );
        // A selection with no path equivalent is not guessed at: an alias has
        // no destructure form, and `SELECT author.name` nests under `author`
        // rather than making the flat key a destructure would.
        for query in [
            "SELECT ->wrote->(SELECT title AS t FROM post) AS p FROM user;",
            "SELECT ->wrote->(SELECT string::len(title) FROM post) AS p FROM user;",
        ] {
            assert_eq!(
                findings(query),
                vec!["E6003 surrealql-analyzer can't type what this graph selection projects"],
                "for {query}"
            );
            assert_eq!(projected(query), "array<{ p: any }>", "for {query}");
        }
    }
}
