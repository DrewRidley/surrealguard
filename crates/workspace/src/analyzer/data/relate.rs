//! `RELATE` statement analysis.
//!
//! Owns RELATE-specific analysis: the target table is the *edge* named in
//! the traversal (`from->edge->to`), and the three endpoints are separate
//! spanned positions so edge-endpoint invariants can point at each.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;
use surrealguard_syntax::source::SourceId;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::data::mutation;
use crate::schema::SchemaIndex;
use crate::statement_env::StatementEnv;

pub fn analyze_relate(ctx: &mut AnalysisContext<'_>, stmt: &ast::RelateStmt) -> Kind {
    relate_response_kind(
        stmt,
        ctx.source(),
        ctx.source_text(),
        ctx.schema(),
        ctx.env(),
    )
}

pub(crate) fn relate_response_kind(
    stmt: &ast::RelateStmt,
    source: &SourceId,
    text: &str,
    schema: &SchemaIndex,
    env: &StatementEnv,
) -> Kind {
    let Some(table_name) = mutation::source_table_name(stmt.edge.as_ref()) else {
        return Kind::Any;
    };
    let Some(table) = schema.tables.get(&table_name) else {
        return Kind::Any;
    };

    mutation::response_kind_for_target(
        stmt.only,
        stmt.ret.as_ref(),
        table,
        source,
        text,
        schema,
        env,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_syntax::lower::lower_statement;
    use surrealguard_syntax::parse::parse_source;

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
        relate_response_kind(&stmt, parsed.source_id(), parsed.text(), schema, &env)
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
