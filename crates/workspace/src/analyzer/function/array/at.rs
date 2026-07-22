//! `array::at` function analysis: `array::at(array, index) -> element`.
//!
//! Returns the element at `index` (negative indices count from the end).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_array_at(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Array, ParamKind::Numeric],
            return_kind: ReturnKind::ArrayElement(0),
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
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Array, ParamKind::Numeric],
            return_kind: ReturnKind::ArrayElement(0),
        }
    }

    #[test]
    fn returns_element_kind() {
        assert_eq!(
            evaluate(
                &signature(),
                &[Kind::Array(Box::new(Kind::String), None), Kind::Int]
            ),
            Kind::String
        );
    }

    #[test]
    fn missing_index_still_infers_the_element_kind() {
        // The absent index violates arity, but the element kind is knowable
        // from the array argument alone.
        assert_eq!(
            evaluate(&signature(), &[Kind::Array(Box::new(Kind::String), None)]),
            Kind::String
        );
    }
}
