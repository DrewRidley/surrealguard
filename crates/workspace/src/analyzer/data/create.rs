//! `CREATE` statement analysis.
//!
//! Owns CREATE-specific analysis; the target is the first source after
//! the statement keyword. CREATE-specific invariants (e.g. payload
//! assignability) belong here.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;
use surrealguard_syntax::source::SourceId;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::data::mutation;
use crate::schema::SchemaIndex;
use crate::statement_env::StatementEnv;

pub fn analyze_create(ctx: &mut AnalysisContext<'_>, stmt: &ast::CreateStmt) -> Kind {
    create_response_kind(
        stmt,
        ctx.source(),
        ctx.source_text(),
        ctx.schema(),
        ctx.env(),
    )
}

pub(crate) fn create_response_kind(
    stmt: &ast::CreateStmt,
    source: &SourceId,
    text: &str,
    schema: &SchemaIndex,
    env: &StatementEnv,
) -> Kind {
    let Some(table_name) = mutation::source_table_name(stmt.targets.first()) else {
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
            "CreateStatement",
        )
        .expect("create statement exists");
        let ast::Statement::Create(stmt) = lower_statement(node, parsed.text()).node else {
            panic!("expected create statement");
        };
        let env = StatementEnv::default();
        create_response_kind(&stmt, parsed.source_id(), parsed.text(), schema, &env)
    }

    #[test]
    fn infers_array_of_full_table_rows_by_default() {
        let schema_parsed = parse_source(
            SourceId::new("schema"),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;",
        )
        .expect("schema should parse");
        let schema = extract_schema(&[schema_parsed]).schema;

        let kind = analyze(&schema, "CREATE person;");

        let Kind::Array(element, None) = kind else {
            panic!("expected unbounded array kind, got {kind:?}");
        };
        let Kind::Literal(surrealdb_types::KindLiteral::Object(fields)) = *element else {
            panic!("expected object literal element");
        };
        assert_eq!(fields["name"], Kind::String);
    }

    #[test]
    fn unknown_target_table_is_poison() {
        let schema = SchemaIndex::default();

        let kind = analyze(&schema, "CREATE ghost;");

        assert_eq!(kind, Kind::Any);
    }
}
