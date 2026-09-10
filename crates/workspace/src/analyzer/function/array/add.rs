//! `array::add` function analysis: `array::add(array, value) -> array`.
//!
//! Appends `value` only if it is not already present; the array kind is
//! preserved.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 2,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Array, ParamKind::Any],
        return_kind: ReturnKind::SameAsArg(0),
    }
}

pub(crate) fn analyze_array_add(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
