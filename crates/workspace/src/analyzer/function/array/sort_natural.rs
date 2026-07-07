//! `array::sort_natural` function analysis: `array::sort_natural(array, [direction]) -> array`.
//!
//! Sorts using natural ordering; the optional second argument is a sort
//! direction and the array kind is preserved.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_array_sort_natural(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 1,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Array, ParamKind::Any],
            return_kind: ReturnKind::SameAsArg(0),
        },
        args,
    )
}
