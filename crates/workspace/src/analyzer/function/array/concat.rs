//! `array::concat` function analysis: `array::concat(array, array, ...) -> array`.
//!
//! Concatenates two or more arrays; the first array's kind is preserved.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_array_concat(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 2,
            max_args: None,
            arg_kinds: vec![ParamKind::Array],
            return_kind: ReturnKind::SameAsArg(0),
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
            max_args: None,
            arg_kinds: vec![ParamKind::Array],
            return_kind: ReturnKind::SameAsArg(0),
        }
    }

    #[test]
    fn returns_first_array_kind() {
        let array = Kind::Array(Box::new(Kind::Int), None);
        assert_eq!(
            evaluate(&signature(), &[array.clone(), array.clone(), array.clone()]),
            array
        );
    }

    #[test]
    fn mistaken_second_argument_still_infers_from_the_first() {
        assert_eq!(
            evaluate(
                &signature(),
                &[Kind::Array(Box::new(Kind::Int), None), Kind::Int]
            ),
            Kind::Array(Box::new(Kind::Int), None)
        );
    }
}
