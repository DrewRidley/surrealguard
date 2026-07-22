//! `CREATE` statement analysis.
//!
//! Owns CREATE-specific analysis; the target is the first source after
//! the statement keyword. CREATE-specific invariants (e.g. payload
//! assignability) belong here.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::data::mutation;

pub(crate) fn analyze_create(ctx: &mut AnalysisContext<'_>, stmt: &ast::CreateStmt) -> Kind {
    create_response_kind(stmt, ctx)
}

pub(crate) fn create_response_kind(stmt: &ast::CreateStmt, ctx: &mut AnalysisContext<'_>) -> Kind {
    mutation::check_relation_write(ctx, stmt.targets.first(), stmt.data.as_ref());
    mutation::check_return_before_on_create(ctx, stmt.ret.as_ref());
    let table_hint = mutation::source_table_name(stmt.targets.first());
    mutation::analyze_expression_positions_for(
        ctx,
        stmt.data.as_ref(),
        None,
        table_hint.as_deref(),
        true,
    );
    let Some(table_name) = mutation::source_table_name(stmt.targets.first()) else {
        return Kind::Any;
    };
    let Some(table) = ctx.schema().tables.get(&table_name) else {
        if let Some(target) = stmt.targets.first() {
            crate::analyzer::data::check_table_reference(ctx, &table_name, target.span);
        }
        return Kind::Any;
    };

    // A CREATE without CONTENT/SET providing a required field fails.
    if let Some(target) = stmt.targets.first() {
        let provided = mutation::provided_field_names(stmt.data.as_ref());
        mutation::check_required_fields(ctx, table, &provided, target.span);
    }

    mutation::response_kind_for_target(stmt.only, stmt.ret.as_ref(), table, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::SchemaIndex;
    use crate::statement_env::StatementEnv;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    use crate::schema::extract_schema;

    fn analyze(schema: &SchemaIndex, query: &str) -> Kind {
        let parsed = parse_source(SourceId::new("query"), query).expect("query should parse");
        let ast::Statement::Create(stmt) =
            surrealguard_syntax::lower::lower_first_statement(&parsed, "CreateStatement")
                .expect("create statement exists")
                .node
        else {
            panic!("expected create statement");
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
        create_response_kind(&stmt, &mut ctx)
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
