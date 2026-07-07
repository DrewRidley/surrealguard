//! `api::invoke` function analysis: `api::invoke(path: string[, options]) -> any`.
//!
//! Invokes a user-defined API route. The response shape is determined at
//! runtime by the route handler, so the return kind is genuinely `any`; only
//! the arguments (a path string plus an optional options object) are checked.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_api_invoke(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 1,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Exact(Kind::String), ParamKind::Object],
            return_kind: ReturnKind::Fixed(Kind::Object),
        },
        args,
    )
}
