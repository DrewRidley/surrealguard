//! `http::head` function analysis: `http::head(url: string, headers: option<object>) -> any`.
//!
//! Returns `any`: the runtime result depends on the remote server's response.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_http_head(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 1,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Exact(Kind::String), ParamKind::Object],
            return_kind: ReturnKind::Fixed(Kind::None),
        },
        args,
    )
}
