//! `type::field` function analysis: `type::field(path) -> field value`.
//!
//! The path argument is a *value*, but when it is a string literal — or a
//! `LET` binding tracing back to one — the field it names is known
//! statically and resolves against the row context:
//! `type::field('name.first')` has `name.first`'s declared kind.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::const_value_arg;

pub(crate) fn analyze_type_field(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = args;
    let path = match const_value_arg(ctx, call, 0) {
        Some(surrealdb_types::Value::String(path)) => path,
        Some(other) => {
            // The value is known and provably not a field path.
            if let Some(arg) = call.args.first() {
                let span =
                    surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), arg.span);
                ctx.emit(surrealguard_diagnostics::catalog::finding(
                    span,
                    5005,
                    format!("type::field expects a field-path string, found `{other:?}`"),
                ));
            }
            return Kind::Any;
        }
        None => {
            // An unbound parameter is constrained to the table's field
            // paths — a typed host narrows it to a literal union.
            if let Some(ast::Expr::Param(param)) = call.args.first().map(|arg| &arg.node) {
                if let Some(table) = ctx.row_table() {
                    let paths: Vec<surrealdb_types::Value> = table
                        .fields
                        .keys()
                        .map(|path| surrealdb_types::Value::String(path.clone()))
                        .collect();
                    let span = surrealguard_syntax::span::SourceSpan::new(
                        ctx.source().clone(),
                        call.args[0].span,
                    );
                    ctx.constrain_param(
                        param,
                        span,
                        Kind::String,
                        (!paths.is_empty()).then_some(crate::analysis::ValueDomain::OneOf(paths)),
                    );
                }
            }
            return Kind::Any;
        }
    };
    match field_kind_for_path(ctx, &path) {
        Some(kind) => kind,
        None => {
            if ctx.row_table().is_some() {
                if let Some(arg) = call.args.first() {
                    let span =
                        surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), arg.span);
                    let table = ctx.row_table().map(|t| t.name.clone()).unwrap_or_default();
                    ctx.emit(surrealguard_diagnostics::catalog::finding(
                        span,
                        5005,
                        format!("`{path}` is not a field of table `{table}`"),
                    ));
                }
            }
            Kind::Any
        }
    }
}

pub(crate) fn field_kind_for_path(ctx: &AnalysisContext<'_>, path: &str) -> Option<Kind> {
    let table = ctx.row_table()?;
    let segments: Vec<String> = path.split('.').map(str::to_string).collect();
    crate::analyzer::data::select::kind_for_path(table, &segments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_diagnostics::Finding;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    use crate::analyzer::statement::analyze_lowered_statement;
    use crate::schema::extract_schema;

    #[test]
    fn resolves_let_bound_path_arrays_through_type_fields() {
        // The const-value channel is universal: an array literal bound by
        // LET flows through `$param` into `type::fields`, producing the
        // tuple of the named fields' kinds.
        let schema_parsed = parse_source(
            SourceId::new("schema"),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD title ON person TYPE string;\nDEFINE FIELD age ON person TYPE int;",
        )
        .expect("schema parses");
        let schema = extract_schema(&[schema_parsed]).schema;

        let parsed = parse_source(
            SourceId::new("query"),
            "LET $paths = ['title', 'age'];\nSELECT type::fields($paths) FROM person;",
        )
        .expect("query parses");
        let script = surrealguard_syntax::lower::lower(&parsed);
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(
            &schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
        );

        let mut kinds = Vec::new();
        for statement in &script.statements {
            kinds.push(analyze_lowered_statement(&mut ctx, statement));
        }

        let Some(Kind::Array(element, _)) = kinds.last().cloned() else {
            panic!("expected array kind, got {kinds:?}");
        };
        let Kind::Literal(surrealdb_types::KindLiteral::Object(fields)) = *element else {
            panic!("expected object literal element");
        };
        assert_eq!(
            fields["type::fields($paths)"],
            Kind::Literal(surrealdb_types::KindLiteral::Array(vec![
                Kind::String,
                Kind::Int
            ]))
        );
    }

    #[test]
    fn resolves_let_bound_field_paths_against_the_row() {
        let schema_parsed = parse_source(
            SourceId::new("schema"),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD title ON person TYPE string;\nDEFINE FIELD name.first ON person TYPE string;\nDEFINE FIELD name.last ON person TYPE string;",
        )
        .expect("schema parses");
        let schema = extract_schema(&[schema_parsed]).schema;

        let parsed = parse_source(
            SourceId::new("query"),
            "LET $param = 'name.first';\nSELECT type::field($param) FROM person;",
        )
        .expect("query parses");
        let script = surrealguard_syntax::lower::lower(&parsed);
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(
            &schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
        );

        // Thread the statements in order: the LET binds, the SELECT reads.
        let mut kinds = Vec::new();
        for statement in &script.statements {
            kinds.push(analyze_lowered_statement(&mut ctx, statement));
        }

        let Some(Kind::Array(element, _)) = kinds.last().cloned() else {
            panic!("expected array kind, got {kinds:?}");
        };
        let Kind::Literal(surrealdb_types::KindLiteral::Object(fields)) = *element else {
            panic!("expected object literal element");
        };
        assert_eq!(fields["type::field($param)"], Kind::String);
    }
}
