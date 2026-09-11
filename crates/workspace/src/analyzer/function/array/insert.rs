//! `array::insert` function analysis: `array::insert(array, value, [index]) -> array`.
//!
//! Inserts `value` at the optional `index` (appends when omitted); the array
//! kind is preserved.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 2,
        max_args: Some(3),
        arg_kinds: vec![ParamKind::Array, ParamKind::Any, ParamKind::Numeric],
        return_kind: ReturnKind::SameAsArg(0),
    }
}

pub(crate) fn analyze_array_insert(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
