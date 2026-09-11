//! `array::slice` function analysis: `array::slice(array, [start], [len]) -> array`.
//!
//! `start` and `len` are optional; the result preserves the input array's
//! kind.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(3),
        arg_kinds: vec![ParamKind::Array, ParamKind::Numeric, ParamKind::Numeric],
        return_kind: ReturnKind::SameAsArg(0),
    }
}

pub(crate) fn analyze_array_slice(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
