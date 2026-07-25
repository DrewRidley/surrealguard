//! `api::res::headers` function analysis: signature unverified.
//!
//! An API-definition middleware entry, like [`super::timeout`]: it wraps the
//! next handler rather than returning a value-typed result, and its real
//! arity and wrapped kind are undocumented. Recognised so a valid
//! `DEFINE API ... MIDDLEWARE api::res::headers(...)` is not a false "unknown
//! function", but left `any` rather than guessing a signature that could
//! reject a valid call.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_api_res_headers(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call, args);
    Kind::Any
}
