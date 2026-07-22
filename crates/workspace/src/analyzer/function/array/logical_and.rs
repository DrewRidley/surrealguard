//! `array::logical_and` function analysis:
//! `array::logical_and(array, array) -> array`.
//!
//! Combines the two arrays element-wise with logical AND, returning the
//! operand values (not just booleans); the first array's kind is preserved.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_array_logical_and(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Array, ParamKind::Array],
            return_kind: ReturnKind::SameAsArg(0),
        },
        args,
    )
}
