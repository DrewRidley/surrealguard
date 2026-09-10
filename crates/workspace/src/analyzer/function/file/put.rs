//! `file::put` function analysis: `file::put(file, value: any) -> any`.
//!
//! Writes `value` to the file. Result kind is a side-effect status that is
//! not stable enough to commit to, so `any` is inferred; the signature still
//! enforces the (file, value) arity.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 2,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Any, ParamKind::Any],
        return_kind: ReturnKind::Fixed(Kind::None),
    }
}

pub(crate) fn analyze_file_put(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
