//! `set::flatten` function analysis: `set::flatten(set) -> set`.
//!
//! Flattens one level of nested collections. The signature table can't express
//! the element-of-element unwrap, so the return kind is derived here: a set of
//! `T` is produced from a `set<set<T>>`/`set<array<T>>`, otherwise the element
//! kind is preserved.

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
        return_kind: ReturnKind::Fixed(Kind::Set(Box::new(Kind::Any), None)),
    }
}

pub(crate) fn analyze_set_flatten(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    match args {
        [Kind::Array(element, _) | Kind::Set(element, _)] => match element.as_ref() {
            Kind::Array(inner, _) | Kind::Set(inner, _) => {
                Kind::Set(Box::new((**inner).clone()), None)
            }
            _ => Kind::Set(element.clone(), None),
        },
        _ => Kind::Any,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::test_support;

    #[test]
    fn unwraps_one_nesting_level_into_a_set() {
        test_support::with_ctx(|ctx| {
            let call = test_support::synthetic_call("set::flatten");
            assert_eq!(
                analyze_set_flatten(
                    ctx,
                    &call,
                    &[Kind::Set(
                        Box::new(Kind::Array(Box::new(Kind::Int), None)),
                        None
                    )]
                ),
                Kind::Set(Box::new(Kind::Int), None)
            );
        });
    }
}
