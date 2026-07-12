//! `DEFINE EVENT` analysis.
//!
//! An event targets a known table (1001) and its `$event.<field>` references
//! must resolve on that table (1002). Event bodies run with the context
//! parameters bound: `$event` is the literal union `'CREATE' | 'UPDATE' |
//! 'DELETE'`, and `$before`/`$after`/`$value` carry the table's row type.
//! With those in scope the WHEN condition and THEN body get the ordinary
//! checks (a `WHEN $event = 'CRATE'` typo surfaces through the general
//! operand/comparison rules).

use surrealdb_types::{Kind, KindLiteral};
use surrealguard_syntax::ast;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use crate::analyzer::context::AnalysisContext;
use crate::expression::{ExpressionFact, ExpressionValueClass};

pub fn analyze_define_event(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineEvent) -> Kind {
    let row_kind = ctx
        .schema()
        .tables
        .get(&stmt.table.node)
        .filter(|table| !table.fields.is_empty())
        .map(crate::analyzer::data::select::object_kind_for_all_fields);

    ctx.with_child_env(|ctx| {
        let bind = |ctx: &mut AnalysisContext<'_>, name: &str, kind: Option<Kind>| {
            let span =
                surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), stmt.name.span);
            let mut fact = ExpressionFact::new(span, ExpressionValueClass::Variable);
            fact.kind = kind;
            ctx.define_local(name.to_string(), fact);
        };
        bind(
            ctx,
            "event",
            Some(Kind::Either(vec![
                Kind::Literal(KindLiteral::String("CREATE".into())),
                Kind::Literal(KindLiteral::String("UPDATE".into())),
                Kind::Literal(KindLiteral::String("DELETE".into())),
            ])),
        );
        bind(ctx, "before", row_kind.clone());
        bind(ctx, "after", row_kind.clone());
        bind(ctx, "value", row_kind);

        let table = ctx.schema().tables.get(&stmt.table.node);
        ctx.with_row_table(table, |ctx| {
            if let Some(when) = &stmt.when {
                crate::analyzer::expression::analyze_expr(ctx, when);
            }
            if let Some(then) = &stmt.then {
                crate::analyzer::expression::analyze_expr(ctx, then);
            }
        });
    });

    check_event_references(ctx, stmt);
    Kind::None
}

/// An event's catalog contracts: a known target table (1001) and resolvable
/// `$event.<field>` references on it (1002).
fn check_event_references(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineEvent) {
    if !ctx.schema().tables.contains_key(&stmt.table.node) {
        ctx.emit(surrealguard_diagnostics::catalog::finding(
            SourceSpan::new(ctx.source().clone(), stmt.table.span),
            1001,
            format!(
                "event `{}` targets unknown table `{}`",
                stmt.name.node, stmt.table.node
            ),
        ));
        return;
    }

    let mut refs = Vec::new();
    for clause in [&stmt.when, &stmt.then].into_iter().flatten() {
        collect_event_field_refs(clause, &mut refs);
    }

    let unknown: Vec<(String, ByteRange)> = {
        let table = &ctx.schema().tables[&stmt.table.node];
        refs.iter()
            .filter(|(path, _, _)| !crate::schema::index_field_path_exists_on_table(table, path))
            .map(|(_, text, span)| (text.clone(), *span))
            .collect()
    };
    let source = ctx.source().clone();
    for (text, span) in unknown {
        ctx.emit(surrealguard_diagnostics::catalog::finding(
            SourceSpan::new(source.clone(), span),
            1002,
            format!(
                "event `{}` references unknown field `{}` on table `{}`",
                stmt.name.node, text, stmt.table.node
            ),
        ));
    }
}

/// Collects the `(field-path, dotted-text, span)` of every `$event.<field>`
/// idiom reachable from an expression.
fn collect_event_field_refs(
    expr: &ast::Spanned<ast::Expr>,
    out: &mut Vec<(Vec<String>, String, ByteRange)>,
) {
    match &expr.node {
        ast::Expr::Idiom(idiom) => {
            if let Some(ast::IdiomPart::Start(start)) = idiom.parts.first().map(|part| &part.node) {
                if matches!(&start.node, ast::Expr::Param(name) if name == "event") {
                    let path = crate::schema::idiom_field_path(idiom);
                    if !path.is_empty() {
                        out.push((path.clone(), path.join("."), expr.span));
                    }
                }
                collect_event_field_refs(start, out);
            }
        }
        ast::Expr::Binary { lhs, rhs, .. } => {
            collect_event_field_refs(lhs, out);
            collect_event_field_refs(rhs, out);
        }
        ast::Expr::Prefix { expr: inner, .. } | ast::Expr::Cast { expr: inner, .. } => {
            collect_event_field_refs(inner, out);
        }
        ast::Expr::Call(call) => {
            for arg in &call.args {
                collect_event_field_refs(arg, out);
            }
        }
        ast::Expr::Array(elements) => {
            for element in elements {
                collect_event_field_refs(element, out);
            }
        }
        ast::Expr::Object(fields) => {
            for (_, value) in fields {
                collect_event_field_refs(value, out);
            }
        }
        ast::Expr::Block(block) => collect_event_field_refs_block(block, out),
        ast::Expr::Subquery(stmt) => collect_event_field_refs_stmt(stmt, out),
        _ => {}
    }
}

fn collect_event_field_refs_block(
    block: &ast::Block,
    out: &mut Vec<(Vec<String>, String, ByteRange)>,
) {
    for stmt in &block.statements {
        collect_event_field_refs_stmt(stmt, out);
    }
}

fn collect_event_field_refs_stmt(
    stmt: &ast::Spanned<ast::Statement>,
    out: &mut Vec<(Vec<String>, String, ByteRange)>,
) {
    use ast::Statement as S;
    match &stmt.node {
        S::Return(s) => {
            if let Some(value) = &s.value {
                collect_event_field_refs(value, out);
            }
        }
        S::Throw(s) => {
            if let Some(value) = &s.value {
                collect_event_field_refs(value, out);
            }
        }
        S::Let(s) => collect_event_field_refs(&s.value, out),
        S::Expr(e) => collect_event_field_refs(e, out),
        S::Block(block) => collect_event_field_refs_block(block, out),
        _ => {}
    }
}
