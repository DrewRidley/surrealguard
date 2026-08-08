//! Top-level analyzer dispatch over the lowered AST.
//!
//! `analyze_lowered_statement` matches exhaustively over
//! `ast::Statement` — adding a variant without routing it is a compile
//! error. Every statement dispatches to its own analyzer; composite
//! constructs (blocks, IF/FOR bodies) re-enter here per child, so
//! per-statement rules stay attached to their own analyzer.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_lowered_statement(
    ctx: &mut AnalysisContext<'_>,
    stmt: &ast::Spanned<ast::Statement>,
) -> Kind {
    match &stmt.node {
        ast::Statement::Select(s) => crate::analyzer::data::select::analyze_select(ctx, s),
        ast::Statement::Create(s) => crate::analyzer::data::create::analyze_create(ctx, s),
        ast::Statement::Update(s) => crate::analyzer::data::update::analyze_update(ctx, s),
        ast::Statement::Upsert(s) => crate::analyzer::data::upsert::analyze_upsert(ctx, s),
        ast::Statement::Delete(s) => crate::analyzer::data::delete::analyze_delete(ctx, s),
        ast::Statement::Insert(s) => crate::analyzer::data::insert::analyze_insert(ctx, s),
        ast::Statement::Relate(s) => crate::analyzer::data::relate::analyze_relate(ctx, s),
        ast::Statement::Define(s) => crate::analyzer::schema::define::analyze_define(ctx, s),
        ast::Statement::Remove(s) => crate::analyzer::schema::remove::analyze_remove(ctx, s),
        ast::Statement::Alter(s) => crate::analyzer::schema::alter::analyze_alter(ctx, s),
        ast::Statement::Let(s) => crate::analyzer::flow::let_stmt::analyze_let(ctx, s),
        ast::Statement::Return(s) => crate::analyzer::flow::return_stmt::analyze_return(ctx, s),
        ast::Statement::IfElse(s) => crate::analyzer::flow::if_else::analyze_if_else(ctx, s),
        ast::Statement::For(s) => crate::analyzer::flow::for_loop::analyze_for_loop(ctx, s),
        ast::Statement::Block(s) => crate::analyzer::flow::block::analyze_block(ctx, s),
        ast::Statement::Throw(s) => crate::analyzer::flow::throw::analyze_throw(ctx, s),
        ast::Statement::Break(s) => {
            crate::analyzer::flow::break_stmt::analyze_break(ctx, s, stmt.span)
        }
        ast::Statement::Continue(s) => {
            crate::analyzer::flow::continue_stmt::analyze_continue(ctx, s, stmt.span)
        }
        ast::Statement::LiveSelect(s) => {
            crate::analyzer::data::live_select::analyze_live_select(ctx, s)
        }
        ast::Statement::Kill(s) => crate::analyzer::data::kill::analyze_kill(ctx, s),
        ast::Statement::Use(s) => crate::analyzer::schema::use_stmt::analyze_use(ctx, s),
        ast::Statement::Info(s) => crate::analyzer::schema::info::analyze_info(ctx, s),
        ast::Statement::Show(s) => crate::analyzer::schema::show::analyze_show(ctx, s),
        ast::Statement::Rebuild(s) => crate::analyzer::schema::rebuild::analyze_rebuild(ctx, s),
        ast::Statement::Begin(s) => crate::analyzer::system::begin::analyze_begin(ctx, s),
        ast::Statement::Cancel(s) => crate::analyzer::system::cancel::analyze_cancel(ctx, s),
        ast::Statement::Commit(s) => crate::analyzer::system::commit::analyze_commit(ctx, s),
        ast::Statement::Sleep(s) => crate::analyzer::system::sleep::analyze_sleep(ctx, s),
        ast::Statement::Option(s) => crate::analyzer::system::option::analyze_option(ctx, s),
        ast::Statement::Expr(e) => crate::analyzer::expression::analyze_expr(ctx, e),
        ast::Statement::Partial(_) => Kind::Any,
    }
}

#[cfg(test)]
mod tests {
    use surrealdb_types::Kind;
    use surrealguard_diagnostics::Finding;
    use surrealguard_syntax::lower::lower;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    use super::analyze_lowered_statement;
    use crate::analyzer::context::AnalysisContext;
    use crate::schema::SchemaIndex;

    #[test]
    fn dispatch_handles_every_parseable_statement_kind() {
        // Coverage: every statement kind lowers and dispatches without
        // panicking; spot checks pin representative values (DDL and flow
        // control produce no value, expressions produce real kinds).
        let cases: &[(&str, Option<Kind>)] = &[
            ("SELECT * FROM user;", Some(Kind::Any)),
            ("CREATE user SET name = 'A';", Some(Kind::Any)),
            ("UPDATE user SET name = 'A';", Some(Kind::Any)),
            ("UPSERT user SET name = 'A';", Some(Kind::Any)),
            ("DELETE user;", Some(Kind::Any)),
            ("INSERT INTO user { name: 'A' };", Some(Kind::Any)),
            ("RELATE user:one->likes->post:one;", Some(Kind::Any)),
            ("LIVE SELECT * FROM user;", Some(Kind::Uuid)),
            ("KILL u'e72bee20-f49b-11ec-b939-0242ac120002';", Some(Kind::None)),
            ("DEFINE TABLE user;", Some(Kind::None)),
            ("DEFINE FIELD name ON user;", Some(Kind::None)),
            ("REMOVE TABLE user;", Some(Kind::None)),
            ("ALTER TABLE user DROP;", Some(Kind::None)),
            ("INFO FOR DB;", Some(Kind::Object)),
            ("USE NS test DB test;", Some(Kind::None)),
            ("REBUILD INDEX name ON user;", Some(Kind::None)),
            ("SHOW CHANGES FOR TABLE user SINCE 0;", None),
            ("IF true { RETURN 1; } ELSE { RETURN 2; };", Some(Kind::Int)),
            ("FOR $item IN [1] { RETURN $item; };", Some(Kind::None)),
            ("LET $name = 'A';", Some(Kind::None)),
            ("RETURN 1;", Some(Kind::Int)),
            ("THROW 'bad';", Some(Kind::None)),
            ("BREAK;", Some(Kind::None)),
            ("CONTINUE;", Some(Kind::None)),
            ("BEGIN TRANSACTION;", Some(Kind::None)),
            ("CANCEL TRANSACTION;", Some(Kind::None)),
            ("COMMIT TRANSACTION;", Some(Kind::None)),
            ("OPTION IMPORT;", Some(Kind::None)),
            ("SLEEP 1s;", Some(Kind::None)),
        ];

        for (query, expected) in cases {
            let parsed = parse_source(SourceId::new("query:statement"), *query)
                .unwrap_or_else(|_| panic!("query should parse: {query}"));
            let script = lower(&parsed);
            let Some(statement) = script.statements.first() else {
                panic!("no statement lowered for {query}");
            };

            let schema = SchemaIndex::default();
            let mut diagnostics: Vec<Finding> = Vec::new();
            let mut ctx = AnalysisContext::new(
                &schema,
                parsed.source_id().clone(),
                parsed.text(),
                &mut diagnostics,
            );
            let kind = analyze_lowered_statement(&mut ctx, statement);
            if let Some(expected) = expected {
                assert_eq!(&kind, expected, "query: {query}");
            }
        }
    }
}
