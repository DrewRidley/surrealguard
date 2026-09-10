//! `array::swap` function analysis: `array::swap(array, from, to) -> array`.
//!
//! Swaps the elements at the two indices (negative indices count from the
//! end); the array kind is preserved.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 3,
        max_args: Some(3),
        arg_kinds: vec![ParamKind::Array, ParamKind::Numeric, ParamKind::Numeric],
        return_kind: ReturnKind::SameAsArg(0),
    }
}

pub(crate) fn analyze_array_swap(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
