//! `set::any` function analysis: `set::any(set) -> bool`.
//!
//! Returns `true` when at least one element of the set is truthy.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(1),
        arg_kinds: vec![ParamKind::Array],
        return_kind: ReturnKind::Fixed(Kind::Bool),
    }
}

pub(crate) fn analyze_set_any(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
