//! `RELATE` statement analysis.
//!
//! Owns RELATE-specific analysis: the target table is the *edge* named in
//! the traversal (`from->edge->to`), and the three endpoints are separate
//! spanned positions so edge-endpoint invariants can point at each.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::data::mutation;

pub fn analyze_relate(ctx: &mut AnalysisContext<'_>, stmt: &ast::RelateStmt) -> Kind {
    relate_response_kind(stmt, ctx)
}

pub(crate) fn relate_response_kind(stmt: &ast::RelateStmt, ctx: &mut AnalysisContext<'_>) -> Kind {
    let table_hint = mutation::source_table_name(stmt.edge.as_ref());
    mutation::analyze_expression_positions(ctx, stmt.data.as_ref(), None, table_hint.as_deref());
    let Some(table_name) = mutation::source_table_name(stmt.edge.as_ref()) else {
        return Kind::Any;
    };
    let Some(table) = ctx.schema().tables.get(&table_name) else {
        if let Some(edge) = &stmt.edge {
            crate::analyzer::data::check_table_reference(ctx, &table_name, edge.span);
        }
        return Kind::Any;
    };

    mutation::response_kind_for_target(stmt.only, stmt.ret.as_ref(), table, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::SchemaIndex;
    use crate::statement_env::StatementEnv;
    use surrealguard_syntax::lower::lower_statement;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    use crate::schema::extract_schema;

    fn analyze(schema: &SchemaIndex, query: &str) -> Kind {
        let parsed = parse_source(SourceId::new("query"), query).expect("query should parse");
        let node = crate::analyzer::test_support::find_first_node(
            parsed.tree().root_node(),
            "RelateStatement",
        )
        .expect("relate statement exists");
        let ast::Statement::Relate(stmt) = lower_statement(node, parsed.text()).node else {
            panic!("expected relate statement");
        };
        let env = StatementEnv::default();
        let mut diagnostics: Vec<surrealguard_diagnostics::Finding> = Vec::new();
        let mut ctx = AnalysisContext::scoped(
            schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
            env,
            None,
        );
        relate_response_kind(&stmt, &mut ctx)
    }

    #[test]
    fn infers_array_of_full_table_rows_by_default() {
        let schema_parsed = parse_source(
            SourceId::new("schema"),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD strength ON likes TYPE float;",
        )
        .expect("schema should parse");
        let schema = extract_schema(&[schema_parsed]).schema;

        let kind = analyze(&schema, "RELATE person:one->likes->post:one;");

        let Kind::Array(element, None) = kind else {
            panic!("expected unbounded array kind, got {kind:?}");
        };
        let Kind::Literal(surrealdb_types::KindLiteral::Object(fields)) = *element else {
            panic!("expected object literal element");
        };
        assert_eq!(fields["strength"], Kind::Float);
    }

    #[test]
    fn unknown_target_table_is_poison() {
        let schema = SchemaIndex::default();

        let kind = analyze(&schema, "RELATE person:one->ghost->post:one;");

        assert_eq!(kind, Kind::Any);
    }
}
