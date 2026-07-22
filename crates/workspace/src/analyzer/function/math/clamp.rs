//! `math::clamp` function analysis: `math::clamp(number, min, max) -> number`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_math_clamp(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 3,
            max_args: Some(3),
            arg_kinds: vec![ParamKind::Numeric, ParamKind::Numeric, ParamKind::Numeric],
            return_kind: ReturnKind::Fixed(Kind::Number),
        },
        args,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::function::signature::evaluate;

    fn signature() -> Signature {
        Signature {
            min_args: 3,
            max_args: Some(3),
            arg_kinds: vec![ParamKind::Numeric, ParamKind::Numeric, ParamKind::Numeric],
            return_kind: ReturnKind::Fixed(Kind::Number),
        }
    }

    #[test]
    fn returns_expected_kind_for_valid_call() {
        assert_eq!(
            evaluate(&signature(), &[Kind::Number, Kind::Number, Kind::Number]),
            Kind::Number
        );
    }
}
