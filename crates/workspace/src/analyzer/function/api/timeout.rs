//! `api::timeout` function analysis: signature unverified.
//!
//! `api::timeout` appears to be an API-definition modifier rather than a
//! value-returning function with a documented signature. Since its real
//! arity and wrapped kind are unclear, this stays `any` rather than
//! guessing a signature that could reject valid calls.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{ReturnKind, Signature};

/// The declared shape, for the catalog; the analyzer body below derives
/// the return kind itself rather than checking calls against this.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 0,
        max_args: None,
        arg_kinds: vec![],
        return_kind: ReturnKind::Fixed(Kind::Any),
    }
}

pub(crate) fn analyze_api_timeout(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call, args);
    Kind::Any
}
