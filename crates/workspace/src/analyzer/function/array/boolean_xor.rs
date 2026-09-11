//! `array::boolean_xor` function analysis:
//! `array::boolean_xor(array, array) -> array<bool>`.
//!
//! Performs an element-wise boolean XOR across the two arrays (padding the
//! shorter one), producing an array of booleans.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 2,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Array, ParamKind::Array],
        return_kind: ReturnKind::Fixed(Kind::Array(Box::new(Kind::Bool), None)),
    }
}

pub(crate) fn analyze_array_boolean_xor(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
