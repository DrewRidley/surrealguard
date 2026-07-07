//! `set::reduce` function analysis:
//! `array::reduce(set, closure) -> accumulated`.
//!
//! The result is the closure's return kind, inferred with its parameters
//! bound to `[accumulator, value, index]` at the call site.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::expression::infer::{closure_return_kind, InferScope};
use crate::analyzer::function::closure_arg;

pub fn analyze_set_reduce(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let element = match args.first() {
        Some(Kind::Array(element, _)) | Some(Kind::Set(element, _)) => (**element).clone(),
        _ => return Kind::Any,
    };
    let (accumulator, closure) = (element.clone(), closure_arg(call, 1));
    let Some(closure) = closure else {
        return Kind::Any;
    };

    let scope = InferScope::from_ctx(ctx);
    closure_return_kind(closure, &[accumulator, element, Kind::Int], &scope).unwrap_or(Kind::Any)
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
            let call = test_support::synthetic_call("set::reduce");
            assert_eq!(
                analyze_set_reduce(ctx, &call, &[Kind::Set(Box::new(Kind::Int), None)]),
                Kind::Any
            );
        })
    }
}
