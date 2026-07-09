//! `DEFINE EVENT` analysis.
//!
//! Event bodies run with the context parameters bound: `$event` is the
//! literal union `'CREATE' | 'UPDATE' | 'DELETE'`, and `$before`/`$after`/
//! `$value` carry the table's row type. With those in scope the WHEN
//! condition and THEN body get the ordinary checks (a `WHEN $event =
//! 'CRATE'` typo surfaces through the general operand/comparison rules).

use surrealdb_types::{Kind, KindLiteral};
use surrealguard_syntax::ast;

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

    Kind::None
}
