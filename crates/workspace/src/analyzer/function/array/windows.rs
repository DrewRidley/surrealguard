//! `array::windows` function analysis: `array::windows(array, size) -> array<array>`.
//!
//! Produces every overlapping window of length `size`, so each element of the
//! result is itself an array of the input's element kind. The signature table
//! can't express that extra level of nesting, so the return kind is derived
//! here.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_array_windows(
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
    fn windows_are_arrays_of_the_input_collection() {
        test_support::with_ctx(|ctx| {
            let call = test_support::synthetic_call("array::windows");
            let input = Kind::Array(Box::new(Kind::Int), Some(4));
            assert_eq!(
                analyze_array_windows(ctx, &call, &[input.clone(), Kind::Int]),
                Kind::Array(Box::new(input), None)
            );
        });
    }
}
