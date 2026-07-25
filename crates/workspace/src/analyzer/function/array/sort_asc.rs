//! `array::sort::asc` function analysis: `array::sort::asc(array) -> array`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_array_sort_asc(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Array],
            return_kind: ReturnKind::SameAsArg(0),
        },
        args,
    )
}
