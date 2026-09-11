//! `array::sort` function analysis: `array::sort(array, [direction]) -> array`.
//!
//! The optional second argument is a sort direction (`true`/`false` or
//! `"asc"`/`"desc"`); the array kind is preserved.

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
        return_kind: ReturnKind::SameAsArg(0),
    }
}

pub(crate) fn analyze_array_sort(
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
    fn preserves_array_kind() {
        let array = Kind::Array(Box::new(Kind::Int), None);
        assert_eq!(evaluate(&signature(), std::slice::from_ref(&array)), array);
    }

    #[test]
    fn passes_the_input_kind_through_even_when_mistaken() {
        // `sort` returns its input; a non-array input is an invariant
        // violation, not a type ambiguity.
        assert_eq!(evaluate(&signature(), &[Kind::Int]), Kind::Int);
    }
}
