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

    // A CREATE without CONTENT/SET providing a required field fails. A
    // payload whose shape isn't statically known (`CONTENT $payload`) is no
    // evidence of a missing field, so it is left alone.
    if let Some(target) = stmt.targets.first() {
        if let Some(provided) = mutation::provided_field_names(ctx, stmt.data.as_ref()) {
            mutation::check_required_fields(ctx, table, &provided, target.span);
        }
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

    fn diagnostics_for(
        schema: &SchemaIndex,
        query: &str,
        env: StatementEnv,
    ) -> Vec<surrealguard_diagnostics::Finding> {
        let parsed = parse_source(SourceId::new("query"), query).expect("query should parse");
        let ast::Statement::Create(stmt) =
            surrealguard_syntax::lower::lower_first_statement(&parsed, "CreateStatement")
                .expect("create statement exists")
                .node
        else {
            panic!("expected create statement");
        };
        let mut diagnostics: Vec<surrealguard_diagnostics::Finding> = Vec::new();
        {
            let mut ctx = AnalysisContext::scoped(
                schema,
                parsed.source_id().clone(),
                parsed.text(),
                &mut diagnostics,
                env,
                None,
            );
            create_response_kind(&stmt, &mut ctx);
        }
        diagnostics
    }

    fn missing_fields(diagnostics: &[surrealguard_diagnostics::Finding]) -> usize {
        diagnostics
            .iter()
            .filter(|finding| finding.code().number() == 2034)
            .count()
    }

    fn person_schema() -> SchemaIndex {
        let parsed = parse_source(
            SourceId::new("schema"),
            "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD name ON person TYPE string;\n\
             DEFINE FIELD age ON person TYPE int;",
        )
        .expect("schema should parse");
        extract_schema(&[parsed]).schema
    }

    /// An env with `$p` bound to a closed object of `keys`.
    fn env_with_object(name: &str, keys: &[&str]) -> StatementEnv {
        use crate::expression::{ExpressionFact, ExpressionValueClass};
        let span = surrealguard_syntax::span::SourceSpan::new(
            SourceId::new("query"),
            surrealguard_syntax::span::ByteRange::new(0, 0).expect("empty range is ordered"),
        );
        let fields: std::collections::BTreeMap<String, Kind> = keys
            .iter()
            .map(|key| ((*key).to_string(), Kind::String))
            .collect();
        let mut fact = ExpressionFact::new(span, ExpressionValueClass::Object);
        fact.kind = Some(Kind::Literal(surrealdb_types::KindLiteral::Object(fields)));
        let mut env = StatementEnv::default();
        env.define_let(name.to_string(), fact);
        env
    }

    #[test]
    fn content_payload_of_unknown_shape_claims_no_missing_field() {
        // `CONTENT $payload` provides an unknown key set — that is not
        // evidence a required field is missing, and reporting one is an
        // error-severity false positive that blocks codegen entirely.
        let schema = person_schema();

        let diagnostics = diagnostics_for(
            &schema,
            "CREATE person CONTENT $payload;",
            StatementEnv::default(),
        );

        assert_eq!(
            missing_fields(&diagnostics),
            0,
            "an opaque payload must not report missing fields: {:?}",
            diagnostics
                .iter()
                .map(|f| f.code().number())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn content_payload_bound_to_a_complete_object_claims_no_missing_field() {
        let schema = person_schema();

        let diagnostics = diagnostics_for(
            &schema,
            "CREATE person CONTENT $p;",
            env_with_object("p", &["name", "age"]),
        );

        assert_eq!(missing_fields(&diagnostics), 0);
    }

    #[test]
    fn content_payload_bound_to_an_incomplete_object_reports_only_what_is_missing() {
        // The other direction: when the shape *is* known, it is read.
        let schema = person_schema();

        let diagnostics = diagnostics_for(
            &schema,
            "CREATE person CONTENT $p;",
            env_with_object("p", &["name"]),
        );

        assert_eq!(missing_fields(&diagnostics), 1);
        assert!(
            diagnostics[0].message().contains("`age`"),
            "expected the missing field to be `age`: {}",
            diagnostics[0].message()
        );
    }

    #[test]
    fn a_required_field_omitted_by_a_visible_payload_still_errors() {
        // Guard against over-suppression: every payload whose keys *are*
        // known must still be checked.
        let schema = person_schema();

        for (query, expected) in [
            ("CREATE person CONTENT { name: 'a' };", 1),
            ("CREATE person SET name = 'a';", 1),
            ("CREATE person CONTENT {};", 2),
            ("CREATE person;", 2),
        ] {
            let diagnostics = diagnostics_for(&schema, query, StatementEnv::default());
            assert_eq!(
                missing_fields(&diagnostics),
                expected,
                "`{query}` should report {expected} missing field(s): {:?}",
                diagnostics
                    .iter()
                    .map(|f| f.code().number())
                    .collect::<Vec<_>>()
            );
        }
    }

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
