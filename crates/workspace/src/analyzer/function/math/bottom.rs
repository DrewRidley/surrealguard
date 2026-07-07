//! `math::bottom` function analysis: `math::bottom(array<number>, count) -> array<number>`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_math_bottom(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Array, ParamKind::Numeric],
            return_kind: ReturnKind::Fixed(Kind::Array(Box::new(Kind::Number), None)),
        },
        args,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signature() -> Signature {
        Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Array, ParamKind::Numeric],
            return_kind: ReturnKind::Fixed(Kind::Array(Box::new(Kind::Number), None)),
        }
    }

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
