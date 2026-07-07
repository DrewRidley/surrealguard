//! `array::find_index` function analysis: `array::find_index(array, value) -> int`.
//!
//! Returns the index of the first matching element (or NONE at runtime).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_array_find_index(
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
            return_kind: ReturnKind::Fixed(Kind::Int),
        },
        args,
    )
}
