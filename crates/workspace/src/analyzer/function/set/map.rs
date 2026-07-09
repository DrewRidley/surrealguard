//! `set::map` function analysis: `set::map(set, closure) -> set`.
//!
//! The element kind of the result is the closure's return kind, inferred
//! with the closure's parameters bound to the call site's element kind (the
//! closure receives `[value, index]`). Mapping preserves the array length.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::expression::infer::closure_return_kind;
use crate::analyzer::function::{check_closure_arity, closure_arg};

pub fn analyze_set_map(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let Some((Kind::Set(element, max_len), closure)) =
        args.first().cloned().zip(closure_arg(call, 1))
    else {
        return Kind::Any;
    };
    check_closure_arity(ctx, call, closure, 2);

    let mapped =
        closure_return_kind(closure, &[(*element).clone(), Kind::Int], ctx).unwrap_or(Kind::Any);
    Kind::Set(Box::new(mapped), max_len)
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
            let call = test_support::synthetic_call("set::map");
            assert_eq!(
                analyze_set_map(ctx, &call, &[Kind::Set(Box::new(Kind::Int), None)]),
                Kind::Any
            );
        })
    }
}
