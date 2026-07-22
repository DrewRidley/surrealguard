//! `array::sort_natural_lexical` function analysis:
//! `array::sort_natural_lexical(array, [direction]) -> array`.
//!
//! Sorts using natural lexical ordering; the optional second argument is a
//! sort direction and the array kind is preserved.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_array_sort_natural_lexical(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 1,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Array, ParamKind::Any],
            return_kind: ReturnKind::SameAsArg(0),
        },
        args,
    )
}
