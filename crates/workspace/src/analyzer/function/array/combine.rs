//! `array::combine` function analysis: `array::combine(array, array) -> array`.
//!
//! Produces every pairwise combination of the two arrays as two-element
//! arrays, so the result element kind is itself an array.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_array_combine(
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
            return_kind: ReturnKind::Fixed(Kind::Array(
                Box::new(Kind::Array(Box::new(Kind::Any), None)),
                None,
            )),
        },
        args,
    )
}
