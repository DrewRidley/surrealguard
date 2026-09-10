//! `http::patch` function analysis: `http::patch(url, ...) -> body`.
//!
//! SurrealDB decodes the response by content type: `application/json`
//! parses to a JSON value, `application/octet-stream` to bytes, `text/*`
//! to a string, and anything else to `NONE` — a bounded union, not `any`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

fn http_body_kind() -> Kind {
    let mut variants = match crate::analyzer::function::json_value_kind() {
        Kind::Either(variants) => variants,
        other => vec![other],
    };
    variants.push(Kind::Bytes);
    variants.push(Kind::None);
    Kind::either(variants)
}

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(3),
        arg_kinds: vec![
            ParamKind::Exact(Kind::String),
            ParamKind::Any,
            ParamKind::Object,
        ],
        return_kind: ReturnKind::Fixed(http_body_kind()),
    }
}

pub(crate) fn analyze_http_patch(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
