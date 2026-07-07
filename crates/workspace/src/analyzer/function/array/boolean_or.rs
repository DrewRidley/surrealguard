//! `array::boolean_or` function analysis:
//! `array::boolean_or(array, array) -> array<bool>`.
//!
//! Performs an element-wise boolean OR across the two arrays (padding the
//! shorter one), producing an array of booleans.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_array_boolean_or(
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
            return_kind: ReturnKind::Fixed(Kind::Array(Box::new(Kind::Bool), None)),
        },
        args,
    )
}
