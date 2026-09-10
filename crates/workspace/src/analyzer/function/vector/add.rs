//! `vector::add` function analysis: `vector::add(array, array) -> array`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 2,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Array, ParamKind::Array],
        return_kind: ReturnKind::SameAsArg(0),
    }
}

pub(crate) fn analyze_vector_add(
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
    fn returns_first_vector_kind() {
        let vector = Kind::Array(Box::new(Kind::Float), None);
        assert_eq!(
            evaluate(&signature(), &[vector.clone(), vector.clone()]),
            vector
        );
    }

    #[test]
    fn passes_the_first_argument_kind_through_even_when_mistaken() {
        assert_eq!(
            evaluate(&signature(), &[Kind::Float, Kind::Float]),
            Kind::Float
        );
    }
}
