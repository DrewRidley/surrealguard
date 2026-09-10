//! `api::req::max_body` function analysis:
//! `api::req::max_body(int | string)`.
//!
//! An API-definition middleware entry (docs: "since v3.3.0") that "caps the
//! size of the raw request body", taking a byte count or a size string such
//! as `"512kb"`. Like [`super::req_body`] it wraps the next handler rather
//! than returning a value, so it infers to `any`; the single argument's kind
//! is checked.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(1),
        arg_kinds: vec![ParamKind::Exact(Kind::either(vec![
            Kind::Int,
            Kind::String,
        ]))],
        return_kind: ReturnKind::Fixed(Kind::Any),
    }
}

pub(crate) fn analyze_api_req_max_body(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
