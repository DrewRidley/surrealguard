//! `array::group` function analysis: `array::group(array) -> array`.
//!
//! Flattens an array of arrays and returns the unique values. Like
//! `array::flatten`, the element-of-element unwrap can't be expressed via the
//! signature table, so the return kind is derived here.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{ParamKind, ReturnKind, Signature};

/// The declared shape, for the catalog; the analyzer body below derives
/// the return kind itself rather than checking calls against this.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(1),
        arg_kinds: vec![ParamKind::Array],
        return_kind: ReturnKind::Fixed(Kind::Array(Box::new(Kind::Any), None)),
    }
}

pub(crate) fn analyze_array_group(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    match args {
        [Kind::Array(element, _) | Kind::Set(element, _)] => match element.as_ref() {
            Kind::Array(inner, _) | Kind::Set(inner, _) => {
                Kind::Array(Box::new((**inner).clone()), None)
            }
            _ => Kind::Array(element.clone(), None),
        },
        _ => Kind::Any,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::test_support;

    #[test]
    fn groups_flatten_nested_collections_and_preserve_flat_ones() {
        test_support::with_ctx(|ctx| {
            let call = test_support::synthetic_call("array::group");
            assert_eq!(
                analyze_array_group(
                    ctx,
                    &call,
                    &[Kind::Array(
                        Box::new(Kind::Array(Box::new(Kind::Int), None)),
                        None
                    )]
                ),
                Kind::Array(Box::new(Kind::Int), None)
            );
            assert_eq!(
                analyze_array_group(ctx, &call, &[Kind::Array(Box::new(Kind::Bool), None)]),
                Kind::Array(Box::new(Kind::Bool), None)
            );
        });
    }
}
