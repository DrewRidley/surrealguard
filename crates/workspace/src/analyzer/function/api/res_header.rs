//! `api::res::header` function analysis: signature unverified.
//!
//! An API-definition middleware entry, like [`super::timeout`]: it wraps the
//! next handler rather than returning a value-typed result, and its real
//! arity and wrapped kind are undocumented. Recognised so a valid
//! `DEFINE API ... MIDDLEWARE api::res::header(...)` is not a false "unknown
//! function", but left `any` rather than guessing a signature that could
//! reject a valid call.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

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

pub(crate) fn analyze_api_res_header(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call, args);
    Kind::Any
}
