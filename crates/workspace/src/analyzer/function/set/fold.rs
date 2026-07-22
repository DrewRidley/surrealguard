//! `set::fold` function analysis:
//! `array::fold(set, init, closure) -> accumulated`.
//!
//! The result is the closure's return kind, inferred with its parameters
//! bound to `[accumulator, value, index]` at the call site.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::expression::infer::closure_return_kind;
use crate::analyzer::function::{check_closure_arity, closure_arg};

pub(crate) fn analyze_set_fold(
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
    use super::*;
    use crate::analyzer::test_support;

    #[test]
    fn without_a_closure_expression_the_result_is_unknown() {
        // Synthetic calls carry no argument expressions, so there is no
        // closure body to infer from.
        test_support::with_ctx(|ctx| {
            let call = test_support::synthetic_call("set::fold");
            assert_eq!(
                analyze_set_fold(ctx, &call, &[Kind::Set(Box::new(Kind::Int), None)]),
                Kind::Any
            );
        });
    }
}
