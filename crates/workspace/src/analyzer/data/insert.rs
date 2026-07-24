//! `INSERT` statement analysis.
//!
//! Owns INSERT-specific analysis; the target follows `INTO`, and the
//! payload has its own forms (`InsertData`) the other mutations lack.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
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
                    match &value.node {
                        // `INSERT INTO t [{...}, {...}]` — each element is a row.
                        ast::Expr::Array(rows) => {
                            for row in rows {
                                mutation::check_payload_object_keys(ctx, table, row);
                            }
                        }
                        _ => mutation::check_payload_object_keys(ctx, table, value),
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
                    let value_kind =
                        crate::analyzer::expression::infer::infer_expression_fact(value, ctx).kind;
                    let Some(table) = row_table else {
                        continue;
                    };
                    let Some(segments) =
                        crate::analyzer::expression::infer::plain_field_segments(&column.node)
                    else {
                        continue;
                    };
                    let Some(column_kind) =
                        crate::analyzer::data::select::kind_for_path(table, &segments)
                    else {
                        crate::analyzer::data::check_field_path(
                            ctx,
                            table,
                            &segments,
                            column.span,
                            1002,
                        );
                        continue;
                    };
                    let Some(value_kind) = value_kind else {
                        continue;
                    };
                    if value_kind != Kind::Any
                        && !crate::kinds::kind_is_assignable_to(&value_kind, &column_kind)
                    {
                        let span = surrealguard_syntax::span::SourceSpan::new(
                            ctx.source().clone(),
                            value.span,
                        );
                        let path = segments.join(".");
                        let mut finding = surrealguard_diagnostics::catalog::finding(
                            span,
                            2001,
                            format!(
                                "`{path}` is declared `{column_kind}`, but this value is `{value_kind}`"
                            ),
                        );
                        if let Some(def) = table.fields.get(&path) {
                            finding = finding.with_related(
                                def.name_span.clone(),
                                format!("`{path}` is defined here"),
                            );
                        }
                        ctx.emit(finding);
                    }
                }
            }
        }
        ast::InsertData::Assignments(assignments) => {
            for (target, value) in assignments {
                crate::analyzer::expression::infer::infer_expression_fact(value, ctx);
                if let Some(table) = row_table {
                    if let Some(segments) =
                        crate::analyzer::expression::infer::plain_field_segments(&target.node)
                    {
                        crate::analyzer::data::check_field_path(
                            ctx,
                            table,
                            &segments,
                            target.span,
                            1002,
                        );
                    }
                }
            }
        }
        ast::InsertData::Partial(_) => {}
    }
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
                let keys = mutation::object_keys(value);
                if !keys.is_empty() {
                    mutation::check_required_fields(ctx, table, &keys, target.span);
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
}
