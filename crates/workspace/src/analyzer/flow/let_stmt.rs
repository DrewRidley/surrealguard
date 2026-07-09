//! `LET` statement analysis.
//!
//! Evaluates the bound value and defines it on the statement environment so
//! later statements in the same scope resolve `$name` to its inferred kind.
//! The statement itself produces no value.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_let(ctx: &mut AnalysisContext<'_>, stmt: &ast::LetStmt) -> Kind {
    // SurrealDB rejects assignment to its context parameters at runtime.
    const PROTECTED: &[&str] = &[
        "auth", "session", "token", "access", "this", "parent", "event", "value", "before",
        "after", "input",
    ];
    if PROTECTED.contains(&stmt.name.node.as_str()) {
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), stmt.name.span);
        ctx.emit(surrealguard_diagnostics::catalog::finding(
            span,
            6007,
            format!(
                "`${}` is a protected parameter and cannot be assigned",
                stmt.name.node
            ),
        ));
    }

    let fact = crate::analyzer::expression::expr_fact(ctx, &stmt.value);
    ctx.define_local(stmt.name.node.clone(), fact);
    Kind::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_diagnostics::Finding;
    use surrealguard_syntax::lower::lower_statement;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    use crate::schema::SchemaIndex;

    #[test]
    fn defines_the_binding_with_the_inferred_kind() {
        let parsed =
            parse_source(SourceId::new("flow:test"), "LET $age = 42;").expect("query parses");
        let node =
            crate::analyzer::flow::tests::first_of_kind(parsed.tree().root_node(), "LetStatement");
        let ast::Statement::Let(stmt) = lower_statement(node, parsed.text()).node else {
            panic!("expected let statement");
        };
        let schema = SchemaIndex::default();
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(
            &schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
        );

        let kind = analyze_let(&mut ctx, &stmt);

        assert_eq!(kind, Kind::None);
        assert_eq!(
            ctx.env().let_fact("age").and_then(|fact| fact.kind.clone()),
            Some(Kind::Int)
        );
    }
}
