//! `array::insert` function analysis: `array::insert(array, value, [index]) -> array`.
//!
//! Inserts `value` at the optional `index` (appends when omitted); the array
//! kind is preserved.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_array_insert(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 2,
            max_args: Some(3),
            arg_kinds: vec![ParamKind::Array, ParamKind::Any, ParamKind::Numeric],
            return_kind: ReturnKind::SameAsArg(0),
        },
        args,
    )
}
