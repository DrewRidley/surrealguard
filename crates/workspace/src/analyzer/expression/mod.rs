//! Expression analyzers.
//!
//! [`infer`] holds the AST-native fact-inference engine; [`analyze_expr`] is
//! the context-carrying entry that adds what pure inference can't do:
//! dispatching builtin function calls (with evaluated argument kinds)
//! through `analyzer::function`, and recording external parameter uses.
//!

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
        let args: Vec<Kind> = call.args.iter().map(|arg| analyze_expr(ctx, arg)).collect();
        return crate::analyzer::function::analyze_builtin_function(ctx, call, &args);
    }

    expr_fact(ctx, expr).kind.unwrap_or(Kind::Any)
}

/// [`analyze_expr`], but returning the full fact.
pub fn expr_fact(ctx: &mut AnalysisContext<'_>, expr: &ast::Spanned<ast::Expr>) -> ExpressionFact {
    let source = ctx.source().clone();
    let span = surrealguard_syntax::span::SourceSpan::new(source.clone(), expr.span);

    // Blocks thread an environment through their statements — that is the
    // dispatcher's job, so route through it rather than pure inference.
    if let ast::Expr::Block(block) = &expr.node {
        let kind =
            ctx.with_child_env(|ctx| crate::analyzer::flow::block::analyze_block(ctx, block));
        return ExpressionFact::new(span, crate::expression::ExpressionValueClass::Block)
            .with_kind(kind);
    }

    let fact = {
        let scope = infer::InferScope {
            source: &source,
            text: ctx.source_text(),
            schema: ctx.schema(),
            row_table: ctx.row_table(),
            env: ctx.env(),
        };
        infer::infer_expression_fact(expr, &scope)
    };
    for param in &fact.dependencies.params {
        ctx.record_param_use(param.clone(), fact.span.clone());
    }
    fact
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_diagnostics::Finding;
    use surrealguard_syntax::lower::lower_expr;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    use crate::schema::{extract_schema, SchemaIndex};

    fn find_first<'tree>(
        node: tree_sitter::Node<'tree>,
        kind: &str,
    ) -> Option<tree_sitter::Node<'tree>> {
        if node.kind() == kind {
            return Some(node);
        }
        let mut cursor = node.walk();
        let found = node
            .children(&mut cursor)
            .find_map(|child| find_first(child, kind));
        found
    }

    fn analyze(schema: &SchemaIndex, query: &str, node_kind: &str) -> Kind {
        let parsed = parse_source(SourceId::new("expr:test"), query).expect("query parses");
        let node = find_first(parsed.tree().root_node(), node_kind)
            .unwrap_or_else(|| panic!("no {node_kind} in {query:?}"));
        let lowered = lower_expr(node, parsed.text());
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
        let node =
            find_first(parsed.tree().root_node(), "FunctionCall").expect("function call node");
        let lowered = lower_expr(node, parsed.text());
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
        let node = find_first(parsed.tree().root_node(), "Block").expect("block node");
        let lowered = lower_expr(node, parsed.text());
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
        let node = find_first(parsed.tree().root_node(), "BinaryExpression")
            .expect("binary expression node");
        let lowered = lower_expr(node, parsed.text());
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
