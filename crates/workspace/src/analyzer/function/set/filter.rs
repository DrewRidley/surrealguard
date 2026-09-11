//! `set::filter` function analysis: `set::filter(set, closure) -> set`.
//!
//! The predicate closure isn't typed yet, but it doesn't need to be for the
//! return type: filtering preserves the input set's type (the length bound
//! is dropped — filtering may remove elements).

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{ParamKind, ReturnKind, Signature};

/// The declared shape, for the catalog; the analyzer body below derives
/// the return kind itself rather than checking calls against this.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 2,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Array, ParamKind::Closure],
        return_kind: ReturnKind::Fixed(Kind::Set(Box::new(Kind::Any), None)),
    }
}

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
