//! `type::array` function analysis: `type::array(array) -> array`.
//!
//! The cast is an identity on collections and a runtime error on anything else
//! (3.2.3: `type::array(1)` is "Could not cast into `array` using input `1`"),
//! so the element kind of the argument is the element kind of the result —
//! `type::array($ints)` is an `array<int>`, not an `array<any>`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(1),
        arg_kinds: vec![ParamKind::Any],
        return_kind: ReturnKind::Fixed(Kind::Array(Box::new(Kind::Any), None)),
    }
}

pub(crate) fn analyze_type_array(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let mut signature = signature();
    signature.return_kind = ReturnKind::Fixed(collected(args));
    apply(ctx, call, &signature, args)
}

/// The argument's elements, as an array. A non-collection argument proves no
/// element kind — the cast fails at runtime rather than wrapping — so the
/// result is the unconstrained `array`.
///
/// The length bound survives: `array<int, 3>` re-cast is still at most three
/// elements, and a set of at most three yields at most three.
fn collected(args: &[Kind]) -> Kind {
    match args.first() {
        Some(Kind::Array(element, max_len) | Kind::Set(element, max_len)) => {
            Kind::Array(element.clone(), *max_len)
        }
        _ => Kind::Array(Box::new(Kind::Any), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_argument_element_kind() {
        assert_eq!(
            collected(&[Kind::Array(Box::new(Kind::Int), Some(2))]),
            Kind::Array(Box::new(Kind::Int), Some(2))
        );
        assert_eq!(
            collected(&[Kind::Set(Box::new(Kind::String), None)]),
            Kind::Array(Box::new(Kind::String), None)
        );
    }

    #[test]
    fn a_non_collection_argument_proves_no_element_kind() {
        assert_eq!(
            collected(&[Kind::Int]),
            Kind::Array(Box::new(Kind::Any), None)
        );
        assert_eq!(collected(&[]), Kind::Array(Box::new(Kind::Any), None));
    }
}
