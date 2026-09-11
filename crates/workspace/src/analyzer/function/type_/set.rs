//! `type::set` function analysis: `type::set(array) -> set`.
//!
//! Like [`super::array`], the cast only accepts a collection (3.2.3:
//! `type::set(1)` is "Could not cast into `set` using input `1`"), so the
//! argument's element kind is the result's. Only the duplicates go —
//! `type::set([1, 2, 2])` is `{1, 2}` — which changes how many elements there
//! are, never what they are.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(1),
        arg_kinds: vec![ParamKind::Any],
        return_kind: ReturnKind::Fixed(Kind::Set(Box::new(Kind::Any), None)),
    }
}

pub(crate) fn analyze_type_set(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let mut signature = signature();
    signature.return_kind = ReturnKind::Fixed(deduplicated(args));
    apply(ctx, call, &signature, args)
}

/// The argument's elements, as a set. The length bound is a *maximum*, and
/// dropping duplicates only ever lowers the count, so it carries over.
fn deduplicated(args: &[Kind]) -> Kind {
    match args.first() {
        Some(Kind::Array(element, max_len) | Kind::Set(element, max_len)) => {
            Kind::Set(element.clone(), *max_len)
        }
        _ => Kind::Set(Box::new(Kind::Any), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_argument_element_kind() {
        assert_eq!(
            deduplicated(&[Kind::Array(Box::new(Kind::Int), Some(3))]),
            Kind::Set(Box::new(Kind::Int), Some(3))
        );
    }

    #[test]
    fn a_non_collection_argument_proves_no_element_kind() {
        assert_eq!(
            deduplicated(&[Kind::String]),
            Kind::Set(Box::new(Kind::Any), None)
        );
    }
}
