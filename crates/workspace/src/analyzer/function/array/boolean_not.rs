//! `array::boolean_not` function analysis:
//! `array::boolean_not(array) -> array<bool>`.
//!
//! Negates each element of the array, producing an array of booleans.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_array_boolean_not(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Array],
            return_kind: ReturnKind::Fixed(Kind::Array(Box::new(Kind::Bool), None)),
        },
        args,
    )
}
