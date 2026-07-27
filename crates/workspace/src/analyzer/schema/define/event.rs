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
use crate::analyzer::contract::{Contract, Position};
use crate::expression::{ExpressionFact, ExpressionValueClass};

pub(crate) fn analyze_define_event(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineEvent) -> Kind {
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
        // The base document set, from `context_params`' one table.
        // `$this`/`$self`/`$input` were bound by nobody here, while the hover
        // map inside the same body offered all of them.
        for (name, kind) in crate::context_params::document_param_bindings(&stmt.table.node) {
            bind(ctx, name, Some(kind));
        }
        bind(ctx, "input", row_kind.clone());
        // The event knows a better kind for these three than the base record:
        // the row's full field object. Bound last, so it wins.
        bind(ctx, "before", row_kind.clone());
        bind(ctx, "after", row_kind.clone());
        bind(ctx, "value", row_kind);

        let table = ctx.schema().tables.get(&stmt.table.node);
        ctx.with_row_table(table, |ctx| {
            if let Some(when) = &stmt.when {
                let kind = crate::analyzer::expression::analyze_expr(ctx, when);
                // WHEN gates whether the event fires, so it is a condition like
                // any other. It had no kind contract at all: `WHEN 'CREATE'`
                // (the missing `$event =`) was silent.
                if Contract::condition(Position::EventWhen)
                    .decide(&kind)
                    .is_violation()
                {
                    let span = SourceSpan::new(ctx.source().clone(), when.span);
                    ctx.emit(
                        surrealguard_diagnostics::catalog::finding(
                            span,
                            2005,
                            format!(
                                "this WHEN condition is a `{}`, not a `bool`",
                                crate::render::render_offending(&kind, Some(&Kind::Bool))
                            ),
                        )
                        .with_help("an event fires when its WHEN test is true; it must be a bool"),
                    );
                }
            }
            if let Some(then) = &stmt.then {
                // An event THEN body is in statement position — nothing
                // consumes its value — so a block ending in LET is fine (the
                // value-block check 4017 must not fire). Route a block body
                // straight through the statement path; each inner statement
                // still gets its own analysis (function-arg checks included).
                match &then.node {
                    ast::Expr::Block(block) => {
                        ctx.with_child_env(|ctx| {
                            crate::analyzer::flow::block::analyze_block(ctx, block)
                        });
                    }
                    _ => {
                        crate::analyzer::expression::analyze_expr(ctx, then);
                    }
                }
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
        let finding = surrealguard_diagnostics::catalog::finding(
            SourceSpan::new(ctx.source().clone(), stmt.table.span),
            1001,
            format!(
                "event `{}` targets unknown table `{}`",
                stmt.name.node, stmt.table.node
            ),
        );
        let finding =
            crate::analyzer::data::with_table_suggestion(finding, ctx, &stmt.table.node);
        ctx.emit(finding);
        return;
    }

    let mut refs = Vec::new();
    for clause in [&stmt.when, &stmt.then].into_iter().flatten() {
        collect_event_field_refs(clause, &mut refs);
    }

    let (unknown, table_name_span, field_keys): (Vec<(String, ByteRange)>, SourceSpan, Vec<String>) = {
        let table = &ctx.schema().tables[&stmt.table.node];
        let unknown = refs
            .iter()
            .filter(|(path, _, _)| !crate::schema::index_field_path_exists_on_table(table, path))
            .map(|(_, text, span)| (text.clone(), *span))
            .collect();
        (
            unknown,
            table.name_span.clone(),
            table.fields.keys().cloned().collect(),
        )
    };
    let source = ctx.source().clone();
    for (text, span) in unknown {
        let mut finding = surrealguard_diagnostics::catalog::finding(
            SourceSpan::new(source.clone(), span),
            1002,
            format!(
                "event `{}` references unknown field `{}` on table `{}`",
                stmt.name.node, text, stmt.table.node
            ),
        );
        if let Some(nearest) = crate::suggest::closest(&text, field_keys.iter().map(String::as_str))
        {
            finding = finding.with_help(format!("did you mean `{nearest}`?"));
        }
        finding = finding.with_related(
            table_name_span.clone(),
            format!("`{}` is defined here", stmt.table.node),
        );
        ctx.emit(finding);
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

#[cfg(test)]
mod tests {
    use crate::analysis::{analyze_query, Workspace};

    #[test]
    fn event_unknown_field_1002_carries_suggestion_and_definition_note() {
        let query = concat!(
            "DEFINE TABLE person SCHEMAFULL;\n",
            "DEFINE FIELD name ON person TYPE string;\n",
            "DEFINE EVENT greet ON person WHEN $event = 'CREATE' THEN {\n",
            "  RETURN $event.naem;\n",
            "};\n",
        );
        let mut workspace = Workspace::default();
        let output = analyze_query(&mut workspace, query);
        let finding = output
            .diagnostics
            .iter()
            .find(|finding| finding.code().number() == 1002)
            .expect("a 1002 unknown-field finding on the event path");

        // "did you mean `name`?" over the table's fields.
        assert!(
            finding
                .help()
                .iter()
                .any(|help| help.message == "did you mean `name`?"),
            "expected a did-you-mean suggestion, got: {:?}",
            finding.help()
        );
        // note: pointing at the DEFINE TABLE.
        assert!(
            finding
                .related()
                .iter()
                .any(|note| note.message == "`person` is defined here"),
            "expected a related note at the table definition, got: {:?}",
            finding.related()
        );
    }

    #[test]
    fn event_row_params_carry_the_implicit_id() {
        // `$before`/`$after`/`$value` are full materialized rows, so `id`
        // resolves on them (TG-1). Observed through the assignment contract:
        // `$after.id` is a `record<t>`, not an untyped `any`.
        let query = concat!(
            "DEFINE TABLE t SCHEMAFULL;\n",
            "DEFINE FIELD name ON t TYPE string;\n",
            "DEFINE EVENT ev ON t WHEN $event = 'CREATE' THEN {\n",
            "  UPDATE t SET name = $after.id;\n",
            "};\n",
        );
        let mut workspace = Workspace::default();
        let output = analyze_query(&mut workspace, query);
        let finding = output
            .diagnostics
            .iter()
            .find(|finding| finding.code().number() == 2001)
            .expect("`$after.id` is a record<t>, which is not assignable to a string field");
        assert!(
            finding.message().contains("record<t>"),
            "got: {}",
            finding.message()
        );
    }

    #[test]
    fn event_row_params_on_a_relation_carry_in_and_out() {
        let query = concat!(
            "DEFINE TABLE person SCHEMAFULL;\n",
            "DEFINE FIELD name ON person TYPE string;\n",
            "DEFINE TABLE post SCHEMAFULL;\n",
            "DEFINE FIELD title ON post TYPE string;\n",
            "DEFINE TABLE likes SCHEMAFULL TYPE RELATION FROM person TO post;\n",
            "DEFINE FIELD since ON likes TYPE datetime;\n",
            "DEFINE EVENT ev ON likes WHEN $event = 'CREATE' THEN {\n",
            "  UPDATE person SET name = $after.in;\n",
            "};\n",
        );
        let mut workspace = Workspace::default();
        let output = analyze_query(&mut workspace, query);
        let finding = output
            .diagnostics
            .iter()
            .find(|finding| finding.code().number() == 2001)
            .expect("`$after.in` is a record<person>, not a string");
        assert!(
            finding.message().contains("record<person>"),
            "got: {}",
            finding.message()
        );
    }
}
