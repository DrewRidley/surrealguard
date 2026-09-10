//! `array::clump` function analysis: `array::clump(array, size) -> array<array>`.
//!
//! Splits the array into consecutive chunks of length `size`, so each element
//! of the result is itself an array of the input's element kind. The signature
//! table can't express that extra level of nesting, so the return kind is
//! derived here.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{ParamKind, ReturnKind, Signature};

/// The declared shape, for the catalog; the analyzer body below derives
/// the return kind itself rather than checking calls against this.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 2,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Array, ParamKind::Numeric],
        return_kind: ReturnKind::Fixed(Kind::Array(
            Box::new(Kind::Array(Box::new(Kind::Any), None)),
            None,
        )),
    }
}

pub(crate) fn analyze_array_clump(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    match args {
        [input @ (Kind::Array(_, _) | Kind::Set(_, _)), Kind::Int | Kind::Float | Kind::Decimal | Kind::Number] => {
            Kind::Array(Box::new(input.clone()), None)
        }
        _ => Kind::Any,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::test_support;

    #[test]
    fn clumps_are_arrays_of_the_input_collection() {
        test_support::with_ctx(|ctx| {
            let call = test_support::synthetic_call("array::clump");
            let input = Kind::Array(Box::new(Kind::String), None);
            assert_eq!(
                analyze_array_clump(ctx, &call, &[input.clone(), Kind::Int]),
                Kind::Array(Box::new(input), None)
            );
        });
    }
}
