//! `INSERT` statement analysis.
//!
//! Owns INSERT-specific analysis; the target follows `INTO`, and the
//! payload has its own forms (`InsertData`) the other mutations lack.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::contract::Position;
use crate::analyzer::data::mutation;
use crate::schema::TableDef;

pub(crate) fn analyze_insert(ctx: &mut AnalysisContext<'_>, stmt: &ast::InsertStmt) -> Kind {
    insert_response_kind(stmt, ctx)
}

pub(crate) fn insert_response_kind(stmt: &ast::InsertStmt, ctx: &mut AnalysisContext<'_>) -> Kind {
    mutation::check_relation_insert(ctx, stmt.target.as_ref(), &stmt.data);
    let row_table = mutation::source_table_name(stmt.target.as_ref())
        .and_then(|name| ctx.schema().tables.get(&name));
    check_insert_payload(ctx, &stmt.data, row_table);
    check_on_duplicate_update(ctx, stmt);

    let Some(table_name) = mutation::source_table_name(stmt.target.as_ref()) else {
        return Kind::Any;
    };
    let Some(table) = ctx.schema().tables.get(&table_name) else {
        if let Some(target) = stmt.target.as_ref() {
            crate::analyzer::data::check_table_reference(ctx, &table_name, target.span);
        }
        return Kind::Any;
    };

    check_insert_required_fields(ctx, stmt, table);

    // INSERT has no ONLY modifier — the result is always an array.
    mutation::response_kind_for_target(false, stmt.ret.as_ref(), table, ctx)
}

/// Checks the INSERT payload against the (optional) target table: per-form
/// inference, payload-object keys, and column/value kind agreement (2001,
/// 4004). `row_table` is absent for unknown or dynamic targets.
fn check_insert_payload(
    ctx: &mut AnalysisContext<'_>,
    data: &ast::InsertData,
    row_table: Option<&TableDef>,
) {
    match data {
        ast::InsertData::Values(values) => {
            for value in values {
                crate::analyzer::expression::infer::infer_expression_fact(value, ctx);
                if let Some(table) = row_table {
                    // `INSERT INTO t [{…}, {…}]` — each element is a row, and
                    // each row's keys are checked against the table.
                    for row in mutation::insert_payload_rows(value) {
                        mutation::check_payload_object_keys(
                            ctx,
                            Position::MutationContent,
                            table,
                            row,
                        );
                    }
                }
            }
        }
        ast::InsertData::Rows { rows, misaligned } => {
            if let Some(counts) = misaligned {
                let (values, columns) = counts.node;
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), counts.span);
                ctx.emit(
                    surrealguard_diagnostics::catalog::finding(
                        span,
                        4004,
                        format!(
                            "this INSERT row has {values} {} but {columns} columns",
                            if values == 1 { "value" } else { "values" }
                        ),
                    )
                    .with_help(format!(
                        "give each row exactly {columns} values, one per column"
                    )),
                );
            }
            for row in rows {
                for (column, value) in row {
                    // One column, one value: a field write, held to the same
                    // contracts a `SET` is (2001, 2038). The column resolves
                    // first; a value whose column is unknown, or whose target
                    // table is, is still walked so its own findings emit.
                    let resolved = row_table.and_then(|table| {
                        let segments =
                            crate::analyzer::expression::infer::plain_field_segments(&column.node)?;
                        Some((table, segments))
                    });
                    let Some((table, segments)) = resolved else {
                        crate::analyzer::expression::infer::infer_expression_fact(value, ctx);
                        continue;
                    };
                    let Some(column_kind) =
                        crate::analyzer::data::select::kind_for_path(table, &segments)
                    else {
                        crate::analyzer::expression::infer::infer_expression_fact(value, ctx);
                        crate::analyzer::data::check_field_path(
                            ctx,
                            table,
                            &segments,
                            column.span,
                            1002,
                        );
                        continue;
                    };
                    mutation::check_field_write(
                        ctx,
                        Position::InsertValues,
                        table,
                        &segments,
                        &column_kind,
                        value,
                    );
                }
            }
        }
        ast::InsertData::Partial(_) => {}
    }
}

/// `ON DUPLICATE KEY UPDATE a = 1, b += 2` writes fields of a row that
/// already exists, so the assignments are checked exactly as an `UPDATE …
/// SET` is (unknown field, declared type, READONLY, duplicate targets) —
/// through the same walk, with the row payload checked separately above.
fn check_on_duplicate_update(ctx: &mut AnalysisContext<'_>, stmt: &ast::InsertStmt) {
    if stmt.on_duplicate_update.is_empty() {
        return;
    }
    let table_name = mutation::source_table_name(stmt.target.as_ref());
    let update = ast::DataClause::Set(stmt.on_duplicate_update.clone());
    mutation::analyze_expression_positions_for(
        ctx,
        Some(&update),
        None,
        table_name.as_deref(),
        false,
    );
}

/// Enforces required-field presence once the target table resolves: every
/// VALUES object and every VALUES-with-columns row must supply the table's
/// mandatory fields.
fn check_insert_required_fields(
    ctx: &mut AnalysisContext<'_>,
    stmt: &ast::InsertStmt,
    table: &TableDef,
) {
    let Some(target) = stmt.target.as_ref() else {
        return;
    };
    match &stmt.data {
        ast::InsertData::Values(values) => {
            for value in values {
                // Each row of an array payload is an independent record, so a
                // field missing from two rows is two findings — anchored on
                // each row's own object, never twice on the same span. A
                // single-object payload keeps the target as its anchor, like
                // every other create form.
                let per_row_anchor = matches!(value.node, ast::Expr::Array(_));
                for row in mutation::insert_payload_rows(value) {
                    // A payload whose key set isn't statically known (an unbound
                    // `$row`) is no evidence that a field is missing.
                    let Some(keys) = mutation::payload_field_names(ctx, row) else {
                        continue;
                    };
                    let anchor = if per_row_anchor {
                        row.span
                    } else {
                        target.span
                    };
                    mutation::check_required_fields(ctx, table, &keys, anchor);
                }
            }
        }
        ast::InsertData::Rows { rows, .. } => {
            for row in rows {
                let keys: Vec<String> = row
                    .iter()
                    .filter_map(|(column, _)| {
                        crate::analyzer::expression::infer::plain_field_segments(&column.node)
                            .and_then(|segments| segments.first().cloned())
                    })
                    .collect();
                mutation::check_required_fields(ctx, table, &keys, target.span);
            }
        }
        _ => {}
    }
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
        let ast::Statement::Insert(stmt) =
            surrealguard_syntax::lower::lower_first_statement(&parsed, "InsertStatement")
                .expect("insert statement exists")
                .node
        else {
            panic!("expected insert statement");
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
        insert_response_kind(&stmt, &mut ctx)
    }

    #[test]
    fn infers_array_of_full_table_rows_by_default() {
        let schema_parsed = parse_source(
            SourceId::new("schema"),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;",
        )
        .expect("schema should parse");
        let schema = extract_schema(&[schema_parsed]).schema;

        let kind = analyze(&schema, "INSERT INTO person { name: 'Ada' };");

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

        let kind = analyze(&schema, "INSERT INTO ghost { name: 'Ada' };");

        assert_eq!(kind, Kind::Any);
    }

    fn diagnostics_for(
        schema: &SchemaIndex,
        query: &str,
    ) -> Vec<surrealguard_diagnostics::Finding> {
        let parsed = parse_source(SourceId::new("query"), query).expect("query should parse");
        let ast::Statement::Insert(stmt) =
            surrealguard_syntax::lower::lower_first_statement(&parsed, "InsertStatement")
                .expect("insert statement exists")
                .node
        else {
            panic!("expected insert statement");
        };
        let mut diagnostics: Vec<surrealguard_diagnostics::Finding> = Vec::new();
        {
            let mut ctx = AnalysisContext::scoped(
                schema,
                parsed.source_id().clone(),
                parsed.text(),
                &mut diagnostics,
                StatementEnv::default(),
                None,
            );
            insert_response_kind(&stmt, &mut ctx);
        }
        diagnostics
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

    fn missing_fields(diagnostics: &[surrealguard_diagnostics::Finding]) -> usize {
        diagnostics
            .iter()
            .filter(|finding| finding.code().number() == 2034)
            .count()
    }

    #[test]
    fn insert_payload_of_unknown_shape_claims_no_missing_field() {
        let schema = person_schema();

        let diagnostics = diagnostics_for(&schema, "INSERT INTO person $payload;");

        assert_eq!(
            missing_fields(&diagnostics),
            0,
            "an opaque payload must not report missing fields"
        );
    }

    #[test]
    fn insert_payload_omitting_a_required_field_still_errors() {
        let schema = person_schema();

        let diagnostics = diagnostics_for(&schema, "INSERT INTO person { name: 'Ada' };");

        assert_eq!(missing_fields(&diagnostics), 1);
        assert!(diagnostics
            .iter()
            .any(|finding| finding.message().contains("`age`")));
    }

    // -- array (bulk) payloads: every row is checked ------------------------

    #[test]
    fn array_payload_row_omitting_a_required_field_errors() {
        let schema = person_schema();

        let diagnostics = diagnostics_for(&schema, "INSERT INTO person [{ name: 'Ada' }];");

        assert_eq!(missing_fields(&diagnostics), 1);
        assert!(diagnostics
            .iter()
            .any(|finding| finding.message().contains("`age`")));
    }

    #[test]
    fn array_payload_row_with_unknown_keys_reports_each_key() {
        let schema = person_schema();

        let diagnostics =
            diagnostics_for(&schema, "INSERT INTO person [{ bogus: 1, another: 2 }];");

        let unknown: Vec<_> = diagnostics
            .iter()
            .filter(|finding| finding.code().number() == 1002)
            .collect();
        assert_eq!(unknown.len(), 2, "got: {diagnostics:?}");
        assert!(unknown.iter().any(|f| f.message().contains("`bogus`")));
        assert!(unknown.iter().any(|f| f.message().contains("`another`")));
    }

    #[test]
    fn array_payload_value_of_the_wrong_kind_errors() {
        let schema = person_schema();

        let diagnostics =
            diagnostics_for(&schema, "INSERT INTO person [{ name: 'Ada', age: 'old' }];");

        assert!(
            diagnostics
                .iter()
                .any(|finding| finding.code().number() == 2001
                    && finding.message().contains("`age`")),
            "got: {diagnostics:?}"
        );
    }

    #[test]
    fn a_complete_array_payload_is_silent() {
        let schema = person_schema();

        let diagnostics = diagnostics_for(
            &schema,
            "INSERT INTO person [{ name: 'Ada', age: 36 }, { name: 'Bob', age: 41 }];",
        );

        assert!(diagnostics.is_empty(), "got: {diagnostics:?}");
    }

    #[test]
    fn each_array_row_is_judged_on_its_own() {
        let schema = person_schema();

        // Two rows, one complete and one short: exactly one finding, on the
        // row that is actually short — not one per row, and not one per
        // statement.
        let diagnostics = diagnostics_for(
            &schema,
            "INSERT INTO person [{ name: 'Ada', age: 36 }, { name: 'Bob' }];",
        );

        assert_eq!(missing_fields(&diagnostics), 1, "got: {diagnostics:?}");

        // Both rows short: two findings, one anchored on each row.
        let both = diagnostics_for(
            &schema,
            "INSERT INTO person [{ name: 'Ada' }, { name: 'Bob' }];",
        );
        assert_eq!(missing_fields(&both), 2, "got: {both:?}");
        let spans: Vec<_> = both
            .iter()
            .filter(|finding| finding.code().number() == 2034)
            .map(|finding| finding.span().range())
            .collect();
        assert_ne!(spans[0], spans[1], "each row's finding gets its own span");
    }

    #[test]
    fn an_array_payload_on_a_relation_table_with_in_and_out_is_silent() {
        let parsed = parse_source(
            SourceId::new("schema"),
            "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE TABLE org SCHEMAFULL;\n\
             DEFINE TABLE works_at SCHEMAFULL TYPE RELATION IN person OUT org;",
        )
        .expect("schema should parse");
        let schema = extract_schema(&[parsed]).schema;

        let diagnostics = diagnostics_for(
            &schema,
            "INSERT INTO works_at [{ in: person:ada, out: org:acme }];",
        );

        assert!(diagnostics.is_empty(), "got: {diagnostics:?}");

        // …and a row that omits them is still the 4019 it always was.
        let missing = diagnostics_for(&schema, "INSERT INTO works_at [{ in: person:ada }];");
        assert!(
            missing
                .iter()
                .any(|finding| finding.code().number() == 4019),
            "got: {missing:?}"
        );
    }
}
