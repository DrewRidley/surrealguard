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
//! The kind of a traversal comes from [`super::select`]'s resolvers; this
//! module is the checking-side twin, invoked from the same sites.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

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
    // A traversal must start from records: a leading field prefix that
    // resolves to a non-record kind cannot step anywhere (3009); a record
    // link rebases the traversal onto the link's table.
    let mut source_table = source_table.to_string();
    if let Some((prefix, first_graph)) = leading_field_prefix(idiom) {
        if !prefix.is_empty() {
            if let Some(table) = ctx.schema().tables.get(&source_table) {
                if let Some(kind) = crate::analyzer::data::select::kind_for_path(table, &prefix) {
                    if kind != Kind::Any && !kind_is_recordish(&kind) {
                        let mut finding = surrealguard_diagnostics::catalog::finding(
                            SourceSpan::new(ctx.source().clone(), first_graph),
                            3009,
                            format!(
                                "a graph step can't start from `{}` — `{}` holds no records",
                                prefix.join("."),
                                crate::render_kind(&kind)
                            ),
                        );
                        finding = finding.with_help(
                            "`->`/`<-` traverse from records; this field is not a record link",
                        );
                        ctx.emit(finding);
                        return;
                    }
                    if let Some(target) = single_record_target(&kind) {
                        source_table = target;
                    }
                }
            }
        }
    }
    let source_table = source_table.as_str();

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

    for part in &idiom.parts {
        match &part.node {
            ast::IdiomPart::Graph { dir, step } => {
                let Some(source) = current.clone() else {
                    return;
                };
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
            ast::IdiomPart::Recurse { bounded } if !bounded => {
                ctx.emit(
                    surrealguard_diagnostics::catalog::finding(
                        SourceSpan::new(ctx.source().clone(), part.span),
                        3011,
                        "this recursion has no upper bound and can walk the entire graph"
                            .to_string(),
                    )
                    .with_help("give the range an upper bound, e.g. `{1..5}`"),
                );
            }
            // A field hop after a graph step projects off the current
            // table; the projection resolvers own its kind. Later graph
            // parts continue from wherever the traversal stands.
            _ => {}
        }
    }

    if require_landing {
        if let Some((edge, _)) = &pending_edge {
            if let Some(last) = idiom.parts.last() {
                ctx.emit(
                    surrealguard_diagnostics::catalog::finding(
                        SourceSpan::new(ctx.source().clone(), last.span),
                        3004,
                        format!(
                            "this FROM target stops on the edge `{edge}`, not on a table"
                        ),
                    )
                    .with_help(format!("add a landing step, e.g. `->{edge}->target`")),
                );
            }
        }
    }
}

/// The plain-field segments before the first graph part, with that graph
/// part's span.
fn leading_field_prefix(idiom: &ast::Idiom) -> Option<(Vec<String>, ByteRange)> {
    let mut prefix = Vec::new();
    for part in &idiom.parts {
        match &part.node {
            ast::IdiomPart::Field(name) => prefix.push(name.clone()),
            ast::IdiomPart::Graph { .. } => return Some((prefix, part.span)),
            _ => return None,
        }
    }
    None
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
                    surrealguard_diagnostics::catalog::finding(
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
                surrealguard_diagnostics::catalog::finding(
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
    let mut finding = surrealguard_diagnostics::catalog::finding(span, code, message);
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
            vec!["E3002 relation `wrote` connects `user`->`wrote`->`post`, so this hop cannot land on `comment`"]
        );
        assert_eq!(
            findings("SELECT ->wrote->(post, ghost) FROM user;"),
            vec![
                "E1001 `ghost` is not a defined table",
                "E3002 relation `wrote` connects `user`->`wrote`->`post`, so this hop cannot land on `ghost`",
            ]
        );
        // In traversal position they are still edges, and must be relations.
        assert_eq!(
            findings("SELECT ->(wrote, post) FROM user;"),
            vec!["E3001 `post` can't be traversed — it is not a relation table"]
        );
    }
}
