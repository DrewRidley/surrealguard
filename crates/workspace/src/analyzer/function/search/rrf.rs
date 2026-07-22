//! `search::rrf` function analysis: `search::rrf(array, ...) -> float`.
//!
//! Reciprocal Rank Fusion for hybrid search: takes one or more arrays of
//! per-result rankings and returns the fused relevance score as a float.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_search_rrf(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 1,
            max_args: None,
            arg_kinds: vec![ParamKind::Array],
            return_kind: ReturnKind::Fixed(Kind::Float),
        },
        args,
    )
}
