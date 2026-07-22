//! `array::filter` function analysis: `array::filter(array, closure) -> array`.
//!
//! The predicate closure isn't typed yet, but it doesn't need to be for the
//! return type: filtering preserves the input array's type (the length
//! bound is dropped — filtering may remove elements).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_array_filter(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    match args {
        [Kind::Array(element, _), ..] => Kind::Array(element.clone(), None),
        _ => Kind::Any,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::test_support;

    #[test]
    fn preserves_the_input_array_element_kind() {
        test_support::with_ctx(|ctx| {
            let call = test_support::synthetic_call("array::filter");
            assert_eq!(
                analyze_array_filter(
                    ctx,
                    &call,
                    &[Kind::Array(Box::new(Kind::Int), Some(3)), Kind::Any]
                ),
                Kind::Array(Box::new(Kind::Int), None)
            );
        });
    }
}
