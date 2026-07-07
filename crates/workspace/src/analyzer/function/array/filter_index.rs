//! `array::filter_index` function analysis: `array::filter_index(array, value) -> array<int>`.
//!
//! Returns the indexes of every element equal to `value`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_array_filter_index(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Array, ParamKind::Any],
            return_kind: ReturnKind::Fixed(Kind::Array(Box::new(Kind::Int), None)),
        },
        args,
    )
}
