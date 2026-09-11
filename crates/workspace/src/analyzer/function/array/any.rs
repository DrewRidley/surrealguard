//! `array::any` function analysis: `array::any(array) -> bool`.
//!
//! Returns `true` when at least one element of the array is truthy.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Array, ParamKind::Any],
        return_kind: ReturnKind::Fixed(Kind::Bool),
    }
}

pub(crate) fn analyze_array_any(
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
    fn returns_bool_for_array() {
        assert_eq!(
            evaluate(&signature(), &[Kind::Array(Box::new(Kind::Int), None)]),
            Kind::Bool
        );
    }
}
