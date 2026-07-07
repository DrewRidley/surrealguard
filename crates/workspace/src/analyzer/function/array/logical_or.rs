//! `array::logical_or` function analysis:
//! `array::logical_or(array, array) -> array`.
//!
//! Combines the two arrays element-wise with logical OR, returning the operand
//! values (not just booleans); the first array's kind is preserved.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_array_logical_or(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Array, ParamKind::Array],
            return_kind: ReturnKind::SameAsArg(0),
        },
        args,
    )
}
