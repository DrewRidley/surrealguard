//! `UPSERT` statement analysis.
//!
//! Owns UPSERT-specific analysis. Same response typing as UPDATE today;
//! their invariants will differ (UPSERT may create).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::data::mutation;

pub(crate) fn analyze_upsert(ctx: &mut AnalysisContext<'_>, stmt: &ast::UpsertStmt) -> Kind {
    upsert_response_kind(stmt, ctx)
}

pub(crate) fn upsert_response_kind(stmt: &ast::UpsertStmt, ctx: &mut AnalysisContext<'_>) -> Kind {
    mutation::check_only_on_table(ctx, stmt.only, stmt.targets.first());
    // UPSERT is create-like: a bare `UPSERT table SET ...` (no WHERE) generates
    // a fresh record id rather than rewriting every existing row, so the
    // whole-table-write contract (7009) — which flags unconditional
    // UPDATE/DELETE — does not apply here.
    let table_hint = mutation::source_table_name(stmt.targets.first());
    mutation::analyze_expression_positions_for(
        ctx,
        stmt.data.as_ref(),
        stmt.where_clause.as_ref(),
        table_hint.as_deref(),
        false,
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
        analyze_with_diagnostics(schema, query).0
    }

    fn analyze_with_diagnostics(
        schema: &SchemaIndex,
        query: &str,
    ) -> (Kind, Vec<surrealguard_diagnostics::Finding>) {
        let parsed = parse_source(SourceId::new("query"), query).expect("query should parse");
        let ast::Statement::Upsert(stmt) =
            surrealguard_syntax::lower::lower_first_statement(&parsed, "UpsertStatement")
                .expect("upsert statement exists")
                .node
        else {
            panic!("expected upsert statement");
        };
        let env = StatementEnv::default();
        let mut diagnostics: Vec<surrealguard_diagnostics::Finding> = Vec::new();
        let kind = {
            let mut ctx = AnalysisContext::scoped(
                schema,
                parsed.source_id().clone(),
                parsed.text(),
                &mut diagnostics,
                env,
                None,
            );
            upsert_response_kind(&stmt, &mut ctx)
        };
        (kind, diagnostics)
    }

    #[test]
    fn bare_upsert_is_create_like_and_never_a_whole_table_write() {
        let schema_parsed = parse_source(
            SourceId::new("schema"),
            "DEFINE TABLE settlement SCHEMAFULL;\nDEFINE FIELD amount ON settlement TYPE int;",
        )
        .expect("schema should parse");
        let schema = extract_schema(&[schema_parsed]).schema;

        // No WHERE: UPSERT generates a record, so 7009 must not fire.
        let (_, diagnostics) =
            analyze_with_diagnostics(&schema, "UPSERT settlement SET amount = 1;");
        assert!(
            diagnostics
                .iter()
                .all(|finding| finding.code().number() != 7009),
            "bare UPSERT should not be flagged as a whole-table write: {diagnostics:?}"
        );
    }

    #[test]
    fn infers_array_of_full_table_rows_by_default() {
        let schema_parsed = parse_source(
            SourceId::new("schema"),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;",
        )
        .expect("schema should parse");
        let schema = extract_schema(&[schema_parsed]).schema;

        let kind = analyze(&schema, "UPSERT person SET name = 'A';");

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

        let kind = analyze(&schema, "UPSERT ghost SET name = 'A';");

        assert_eq!(kind, Kind::Any);
    }
}
