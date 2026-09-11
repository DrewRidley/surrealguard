//! `array::flatten` function analysis: `array::flatten(array) -> array`.
//!
//! Flattens one level of nested collections. The signature table can't express
//! the element-of-element unwrap, so the return kind is derived here: an array
//! of `T` is produced from an `array<array<T>>`/`array<set<T>>`, otherwise the
//! element kind is preserved.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

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

pub(crate) fn analyze_array_flatten(
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
    fn unwraps_one_nesting_level_and_preserves_flat_elements() {
        test_support::with_ctx(|ctx| {
            let call = test_support::synthetic_call("array::flatten");
            assert_eq!(
                analyze_array_flatten(
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
                analyze_array_flatten(ctx, &call, &[Kind::Array(Box::new(Kind::String), None)]),
                Kind::Array(Box::new(Kind::String), None)
            );
        });
    }
}
