//! `api::timeout` function analysis: signature unverified.
//!
//! `api::timeout` appears to be an API-definition modifier rather than a
//! value-returning function with a documented signature. Since its real
//! arity and wrapped kind are unclear, this stays `any` rather than
//! guessing a signature that could reject valid calls.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_api_timeout(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call, args);
    Kind::Any
}
