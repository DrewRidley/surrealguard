//! `search::offsets` function analysis:
//! `search::offsets(number [, bool]) -> object`.
//!
//! Returns the positions of the matched terms for a search predicate, keyed
//! by field. The trailing optional `bool` requests partial-match offsets.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Numeric, ParamKind::Exact(Kind::Bool)],
        return_kind: ReturnKind::Fixed(Kind::Object),
    }
}

pub(crate) fn analyze_search_offsets(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
