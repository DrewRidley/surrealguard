//! `math::bottom` function analysis: `math::bottom(array<number>, count) -> array<number>`.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 2,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Array, ParamKind::Numeric],
        return_kind: ReturnKind::Fixed(Kind::Array(Box::new(Kind::Number), None)),
    }
}

pub(crate) fn analyze_math_bottom(
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
            evaluate(
                &signature(),
                &[Kind::Array(Box::new(Kind::Number), None), Kind::Int]
            ),
            Kind::Array(Box::new(Kind::Number), None)
        );
    }
}
