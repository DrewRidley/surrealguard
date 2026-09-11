//! `array::fill` function analysis: `array::fill(array, value, [start], [end]) -> array`.
//!
//! Replaces elements in the optional `[start, end)` range with `value`; the
//! array kind is preserved.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 2,
        max_args: Some(4),
        arg_kinds: vec![
            ParamKind::Array,
            ParamKind::Any,
            ParamKind::Numeric,
            ParamKind::Numeric,
        ],
        return_kind: ReturnKind::SameAsArg(0),
    }
}

pub(crate) fn analyze_array_fill(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
