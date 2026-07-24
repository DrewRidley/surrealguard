//! `array::combine` function analysis: `array::combine(array, array) -> array`.
//!
//! Produces every pairwise combination of the two arrays as two-element
//! arrays, so the result element kind is itself an array. Each pair draws one
//! element from each input, so the inner element kind is the union of the two
//! input element kinds — derivable whenever both inputs are typed arrays.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_array_combine(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let pair_element = pair_element_kind(args);
    apply(
        ctx,
        call,
        &Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Array, ParamKind::Array],
            return_kind: ReturnKind::Fixed(Kind::Array(
                Box::new(Kind::Array(Box::new(pair_element), None)),
                None,
            )),
        },
        args,
    )
}

/// The inner element kind of a combined pair: the union of the two input
/// arrays' element kinds. Stays `Any` when either input isn't a typed array
/// or contributes an `Any` element — no union is narrower than `any`.
fn pair_element_kind(args: &[Kind]) -> Kind {
    let [a, b] = args else {
        return Kind::Any;
    };
    match (array_element(a), array_element(b)) {
        (Some(a_el), Some(b_el)) if a_el != Kind::Any && b_el != Kind::Any => {
            Kind::either(vec![a_el, b_el])
        }
        _ => Kind::Any,
    }
}

/// The element kind of an array/set argument; `None` for any other shape.
fn array_element(kind: &Kind) -> Option<Kind> {
    match kind {
        Kind::Array(element, _) | Kind::Set(element, _) => Some((**element).clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_element_is_the_union_of_input_element_kinds() {
        // `array::combine([1,2], ["a"])` pairs an int with a string.
        assert_eq!(
            pair_element_kind(&[
                Kind::Array(Box::new(Kind::Int), None),
                Kind::Array(Box::new(Kind::String), None),
            ]),
            Kind::Either(vec![Kind::Int, Kind::String])
        );
    }

    #[test]
    fn matching_element_kinds_collapse() {
        assert_eq!(
            pair_element_kind(&[
                Kind::Array(Box::new(Kind::Int), Some(2)),
                Kind::Set(Box::new(Kind::Int), None),
            ]),
            Kind::Int
        );
    }

    #[test]
    fn an_any_element_stays_any() {
        assert_eq!(
            pair_element_kind(&[
                Kind::Array(Box::new(Kind::Any), None),
                Kind::Array(Box::new(Kind::Int), None),
            ]),
            Kind::Any
        );
    }

    #[test]
    fn a_non_array_argument_stays_any() {
        assert_eq!(
            pair_element_kind(&[Kind::Int, Kind::Array(Box::new(Kind::Int), None)]),
            Kind::Any
        );
        assert_eq!(pair_element_kind(&[]), Kind::Any);
    }
}
