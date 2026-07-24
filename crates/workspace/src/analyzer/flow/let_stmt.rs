//! `LET` statement analysis.
//!
//! Evaluates the bound value and defines it on the statement environment so
//! later statements in the same scope resolve `$name` to its inferred kind.
//! The statement itself produces no value.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_let(ctx: &mut AnalysisContext<'_>, stmt: &ast::LetStmt) -> Kind {
    // SurrealDB rejects assignment to its context parameters at runtime.
    const PROTECTED: &[&str] = &[
        "auth", "session", "token", "access", "this", "parent", "event", "value", "before",
        "after", "input",
    ];
    if ctx.env().would_shadow(&stmt.name.node) {
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), stmt.name.span);
        ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                span,
                7002,
                format!(
                    "`${}` is re-bound inside this block; the outer `${}` is unchanged",
                    stmt.name.node, stmt.name.node
                ),
            )
            .with_help("a block introduces a new scope, so this LET does not affect the outer binding")
            .with_help("silence with `W7002 = \"allow\"`"),
        );
    }
    if PROTECTED.contains(&stmt.name.node.as_str()) {
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), stmt.name.span);
        ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                span,
                6007,
                format!(
                    "`${}` is a protected parameter and can't be assigned",
                    stmt.name.node
                ),
            )
            .with_help("protected parameters (`$this`, `$parent`, `$value`, ...) are bound by the engine"),
        );
    }

    let fact = crate::analyzer::expression::expr_fact(ctx, &stmt.value);
    ctx.define_local(stmt.name.node.clone(), fact);
    // Track `LET $t = type::table($x)` so a later `IF $t = 'table'` guard
    // narrows `$x` (indirect record discriminant).
    ctx.set_table_discriminant(
        stmt.name.node.clone(),
        crate::analyzer::flow::narrow::type_table_arg(&stmt.value.node),
    );
    Kind::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_diagnostics::Finding;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    use crate::schema::SchemaIndex;

    #[test]
    fn defines_the_binding_with_the_inferred_kind() {
        let parsed =
            parse_source(SourceId::new("flow:test"), "LET $age = 42;").expect("query parses");
        let ast::Statement::Let(stmt) =
            surrealguard_syntax::lower::lower_first_statement(&parsed, "LetStatement")
                .expect("no LetStatement node in tree")
                .node
        else {
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
