//! `array::reduce` function analysis:
//! `array::reduce(array, closure) -> accumulated`.
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
        min_args: 2,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Array, ParamKind::Closure],
        return_kind: ReturnKind::Fixed(Kind::Any),
    }
}

pub(crate) fn analyze_array_reduce(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let element = match args.first() {
        Some(Kind::Array(element, _) | Kind::Set(element, _)) => (**element).clone(),
        _ => return Kind::Any,
    };
    let (accumulator, closure) = (element.clone(), closure_arg(call, 1));
    let Some(closure) = closure else {
        return Kind::Any;
    };
    check_closure_arity(ctx, call, closure, 3);

    closure_return_kind(closure, &[accumulator, element, Kind::Int], ctx).unwrap_or(Kind::Any)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::test_support;

    #[test]
    fn without_a_closure_expression_the_result_is_unknown() {
        // Synthetic calls carry no argument expressions, so there is no
        // closure body to infer from.
        test_support::with_ctx(|ctx| {
            let call = test_support::synthetic_call("array::reduce");
            assert_eq!(
                analyze_array_reduce(ctx, &call, &[Kind::Array(Box::new(Kind::Int), None)]),
                Kind::Any
            );
        });
    }
}
