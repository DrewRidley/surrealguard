//! `search::score` function analysis: `search::score(number) -> float`.
//!
//! Takes the numeric match reference of a search predicate in the `WHERE`
//! clause and yields that document's relevance score.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(1),
        arg_kinds: vec![ParamKind::Numeric],
        return_kind: ReturnKind::Fixed(Kind::Float),
    }
}

pub(crate) fn analyze_search_score(
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
    fn returns_float_for_numeric_match_ref() {
        assert_eq!(evaluate(&signature(), &[Kind::Int]), Kind::Float);
    }
}
