//! `math::pow` function analysis: `math::pow(number, exponent) -> number`.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 2,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Numeric, ParamKind::Numeric],
        return_kind: ReturnKind::Fixed(Kind::Number),
    }
}

pub(crate) fn analyze_math_pow(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::function::signature::evaluate;

    #[test]
    fn returns_expected_kind_for_valid_call() {
        assert_eq!(
            evaluate(&signature(), &[Kind::Float, Kind::Int]),
            Kind::Number
        );
    }
}
