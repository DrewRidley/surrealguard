//! `file::delete` function analysis: `file::delete(file) -> any`.
//!
//! A side-effecting deletion. Its documented result kind is not stable enough
//! to commit to (`none` vs a status value), so the honest inference is `any`.
//! The signature still enforces the single-file arity.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(1),
        arg_kinds: vec![ParamKind::Any],
        return_kind: ReturnKind::Fixed(Kind::None),
    }
}

pub(crate) fn analyze_file_delete(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
