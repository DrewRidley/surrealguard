//! `set::filter` function analysis: `set::filter(set, closure) -> set`.
//!
//! The predicate closure isn't typed yet, but it doesn't need to be for the
//! return type: filtering preserves the input set's type (the length bound
//! is dropped — filtering may remove elements).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_set_filter(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    match args {
        [Kind::Set(element, _), ..] => Kind::Set(element.clone(), None),
        [Kind::Array(element, _), ..] => Kind::Set(element.clone(), None),
        _ => Kind::Any,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::test_support;

    #[test]
    fn preserves_the_input_set_element_kind() {
        test_support::with_ctx(|ctx| {
            let call = test_support::synthetic_call("set::filter");
            assert_eq!(
                analyze_set_filter(
                    ctx,
                    &call,
                    &[Kind::Set(Box::new(Kind::Int), None), Kind::Any]
                ),
                Kind::Set(Box::new(Kind::Int), None)
            );
        });
    }
}
