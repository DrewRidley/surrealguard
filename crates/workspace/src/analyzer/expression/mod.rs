//! Expression analyzers.
//!
//! `infer` holds the AST-native fact-inference engine; `analyze_expr` is
//! the context-carrying entry that adds what pure inference can't do:
//! dispatching builtin function calls (with evaluated argument kinds)
//! through `analyzer::function`, and recording external parameter uses.
//!

pub(crate) mod check;
pub(crate) mod infer;

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::expression::ExpressionFact;

/// Analyzes a lowered expression with full context: builtin function calls
/// dispatch through `analyzer::function` with evaluated argument kinds, and
/// unresolved external parameters are recorded on the environment.
pub fn analyze_expr(ctx: &mut AnalysisContext<'_>, expr: &ast::Spanned<ast::Expr>) -> Kind {
    if let ast::Expr::Call(call) = &expr.node {
        // The third walk that reaches a call's arguments, and it must mark the
        // cardinality position exactly as the inference and checking walks do
        // — a marker any one walk forgets is a finding the other two suppressed
        // in vain.
        let cardinality =
            crate::analyzer::data::select::reads_only_cardinality(call.path.node.as_str());
        let args: Vec<Kind> = ctx.with_cardinality_position(cardinality, |ctx| {
            call.args.iter().map(|arg| analyze_expr(ctx, arg)).collect()
        });
        return crate::analyzer::function::analyze_builtin_function(ctx, call, &args);
    }

    expr_fact(ctx, expr).kind.unwrap_or(Kind::Any)
}

/// Shape check for a block used as a *value*: a block whose final
/// statement is a `LET` evaluates to NONE — almost always a missing
/// trailing expression (4017). Blocks in statement position are exempt
/// (nothing consumes their value), and an empty `{}` in value position is
/// an empty *object* literal, not a block.
fn check_value_block(
    ctx: &mut AnalysisContext<'_>,
    block: &ast::Block,
    span: surrealguard_syntax::span::ByteRange,
) {
    let _ = span;
    match block.statements.last() {
        None => {}
        Some(last) if matches!(last.node, ast::Statement::Let(_)) => {
            let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), last.span);
            ctx.emit(surrealguard_diagnostics::catalog::finding(
                span,
                4017,
                "block ends with LET, so its value is NONE — return the value instead".to_string(),
            ));
        }
        Some(_) => {}
    }
}

/// [`analyze_expr`], but returning the full fact.
pub fn expr_fact(ctx: &mut AnalysisContext<'_>, expr: &ast::Spanned<ast::Expr>) -> ExpressionFact {
    let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), expr.span);

    // Blocks thread an environment through their statements — that is the
    // dispatcher's job, so route through it rather than pure inference.
    if let ast::Expr::Block(block) = &expr.node {
        check_value_block(ctx, block, expr.span);
        let kind =
            ctx.with_child_env(|ctx| crate::analyzer::flow::block::analyze_block(ctx, block));
        return ExpressionFact::new(span, crate::expression::ExpressionValueClass::Block)
            .with_kind(kind);
    }

    let fact = infer::infer_expression_fact(expr, ctx);
    check::check_value_expression(ctx, expr);
    for param in &fact.dependencies.params {
        if is_context_only_param(param) {
            ctx.emit(surrealguard_diagnostics::catalog::finding(
                fact.span.clone(),
                6005,
                format!("`${param}` only exists inside the construct that binds it"),
            ));
        }
    }
    fact
}

/// Whether a use of `$name` that nothing in scope bound is a finding rather
/// than a host parameter.
///
/// A *document* param exists only where a construct establishes a document, so
/// outside one it names nothing the caller could supply. A *session* param is
/// seeded everywhere and so is never unbound; a *positional* one (`$parent`)
/// is bound by a nesting this analyzer does not model, and claiming it unbound
/// would be inventing a fact.
pub(crate) fn is_context_only_param(name: &str) -> bool {
    crate::context_params::is_document_param(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_diagnostics::Finding;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    use crate::schema::{extract_schema, SchemaIndex};

    fn analyze(schema: &SchemaIndex, query: &str, node_kind: &str) -> Kind {
        let parsed = parse_source(SourceId::new("expr:test"), query).expect("query parses");
        let lowered = surrealguard_syntax::lower::lower_first_expr(&parsed, node_kind)
            .unwrap_or_else(|| panic!("no {node_kind} in {query:?}"));
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(
            schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
        );
        analyze_expr(&mut ctx, &lowered)
    }

    #[test]
    fn dispatches_builtin_calls_with_evaluated_literal_arguments() {
        let schema = SchemaIndex::default();

        assert_eq!(
            analyze(&schema, "RETURN string::len('hi');", "FunctionCall"),
            Kind::Int
        );
        // A mistaken argument violates an invariant; the function's return
        // type is unchanged by the mistake.
        assert_eq!(
            analyze(&schema, "RETURN string::len(42);", "FunctionCall"),
            Kind::Int
        );
    }

    #[test]
    fn literal_object_arguments_satisfy_object_params() {
        // `{ a: 1 }` infers a literal object kind, which must satisfy
        // `ParamKind::Object` rather than failing the argument check.
        let schema = SchemaIndex::default();

        assert_eq!(
            analyze(&schema, "RETURN object::keys({ a: 1 });", "FunctionCall"),
            Kind::Array(Box::new(Kind::String), None)
        );
        assert_eq!(
            analyze(
                &schema,
                "RETURN object::len({ a: 1, b: 2 });",
                "FunctionCall"
            ),
            Kind::Int
        );
    }

    #[test]
    fn resolves_nested_call_arguments_recursively() {
        let schema = SchemaIndex::default();

        assert_eq!(
            analyze(
                &schema,
                "RETURN string::len(string::lowercase('HI'));",
                "FunctionCall"
            ),
            Kind::Int
        );
    }

    #[test]
    fn resolves_schema_backed_field_path_arguments_through_row_table() {
        let schema_parsed = parse_source(
            SourceId::new("expr:schema"),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;",
        )
        .expect("schema parses");
        let schema = extract_schema(&[schema_parsed]).schema;
        let table = schema.tables.get("person").expect("person indexed");

        let parsed = parse_source(SourceId::new("expr:test"), "RETURN string::len(name);")
            .expect("query parses");
        let lowered = surrealguard_syntax::lower::lower_first_expr(&parsed, "FunctionCall")
            .expect("function call node");
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(
            &schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
        );

        let kind = ctx.with_row_table(Some(table), |ctx| analyze_expr(ctx, &lowered));

        assert_eq!(kind, Kind::Int);
    }

    #[test]
    fn user_defined_functions_return_their_declared_kind() {
        let schema_parsed = parse_source(
            SourceId::new("expr:schema"),
            "DEFINE FUNCTION fn::greet($name: string) -> string { RETURN 'hi'; };",
        )
        .expect("schema parses");
        let schema = extract_schema(&[schema_parsed]).schema;

        assert_eq!(
            analyze(&schema, "RETURN fn::greet('a');", "FunctionCall"),
            Kind::String
        );
    }

    #[test]
    fn blocks_evaluate_to_their_trailing_expression() {
        let schema = SchemaIndex::default();
        let parsed = parse_source(SourceId::new("expr:test"), "RETURN { LET $x = 1; $x + 1 };")
            .expect("query parses");
        let lowered =
            surrealguard_syntax::lower::lower_first_expr(&parsed, "Block").expect("block node");
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(
            &schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
        );

        assert_eq!(analyze_expr(&mut ctx, &lowered), Kind::Int);
        // The block's LET does not leak into the outer scope.
        assert!(ctx.env().let_fact("x").is_none());
    }

    #[test]
    fn records_external_param_uses_on_the_environment() {
        let schema = SchemaIndex::default();
        let parsed =
            parse_source(SourceId::new("expr:test"), "RETURN $missing + 1;").expect("query parses");
        let lowered = surrealguard_syntax::lower::lower_first_expr(&parsed, "BinaryExpression")
            .expect("binary expression node");
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(
            &schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
        );

        analyze_expr(&mut ctx, &lowered);

        let params = ctx.env().params();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "missing");
    }
}
