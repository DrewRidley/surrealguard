//! `set::union` function analysis: `set::union(set, set) -> set`.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

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

pub(crate) fn analyze_set_union(
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
    fn returns_first_set_kind() {
        let set = Kind::Set(Box::new(Kind::String), None);
        assert_eq!(evaluate(&signature(), &[set.clone(), set.clone()]), set);
    }

    #[test]
    fn mistaken_second_argument_still_infers_from_the_first() {
        assert_eq!(
            evaluate(
                &signature(),
                &[Kind::Set(Box::new(Kind::String), None), Kind::Int]
            ),
            Kind::Set(Box::new(Kind::String), None)
        );
    }
}
