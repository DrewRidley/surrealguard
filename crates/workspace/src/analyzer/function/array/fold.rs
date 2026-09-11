//! `array::fold` function analysis:
//! `array::fold(array, init, closure) -> accumulated`.
//!
//! The result is the closure's return kind, inferred with its parameters
//! bound to `[accumulator, value, index]` at the call site.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::expression::infer::closure_return_kind;
use crate::analyzer::function::signature::{ParamKind, ReturnKind, Signature};
use crate::analyzer::function::{check_closure_arity, closure_arg};

/// The declared shape, for the catalog; the analyzer body below derives
/// the return kind itself rather than checking calls against this.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 3,
        max_args: Some(3),
        arg_kinds: vec![ParamKind::Array, ParamKind::Any, ParamKind::Closure],
        return_kind: ReturnKind::Fixed(Kind::Any),
    }
}

pub(crate) fn analyze_array_fold(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let element = match args.first() {
        Some(Kind::Array(element, _) | Kind::Set(element, _)) => (**element).clone(),
        _ => return Kind::Any,
    };
    let (accumulator, closure) = (
        args.get(1).cloned().unwrap_or(Kind::Any),
        closure_arg(call, 2),
    );
    let Some(closure) = closure else {
        return Kind::Any;
    };
    check_closure_arity(ctx, call, closure, 3);

    closure_return_kind(closure, &[accumulator, element, Kind::Int], ctx).unwrap_or(Kind::Any)
}

#[cfg(test)]
mod tests {
    use surrealdb_types::Kind;
    use surrealql_analyzer_diagnostics::Finding;
    use surrealql_analyzer_syntax::parse::parse_source;
    use surrealql_analyzer_syntax::source::SourceId;

    use crate::analyzer::context::AnalysisContext;
    use crate::schema::SchemaIndex;

    #[test]
    fn folds_to_the_closure_return_kind_with_the_accumulator_bound() {
        let parsed = parse_source(
            SourceId::new("fn:test"),
            "RETURN array::fold([1, 2], 'start', |$acc, $v| $acc + 'x');",
        )
        .expect("query parses");
        let lowered = surrealql_analyzer_syntax::lower::lower_first_expr(&parsed, "FunctionCall")
            .expect("function call node");
        let schema = SchemaIndex::default();
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(
            &schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
        );

        assert_eq!(
            crate::analyzer::expression::analyze_expr(&mut ctx, &lowered),
            Kind::String
        );
    }
}
