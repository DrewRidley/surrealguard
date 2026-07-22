//! `array::repeat` function analysis: `array::repeat(value, count) -> array`.
//!
//! Builds an array containing `value` repeated `count` times, so the result
//! element kind is the kind of the first argument. The signature table can't
//! wrap an argument kind into an array, so the return kind is derived here.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_array_repeat(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    match args {
        [value, Kind::Int | Kind::Float | Kind::Decimal | Kind::Number] => {
            Kind::Array(Box::new(value.clone()), None)
        }
        _ => Kind::Any,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::test_support;

    #[test]
    fn repeats_the_value_kind_into_an_array() {
        test_support::with_ctx(|ctx| {
            let call = test_support::synthetic_call("array::repeat");
            assert_eq!(
                analyze_array_repeat(ctx, &call, &[Kind::String, Kind::Int]),
                Kind::Array(Box::new(Kind::String), None)
            );
        });
    }
}
