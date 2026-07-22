//! `array::transpose` function analysis: `array::transpose(array) -> array`.
//!
//! Transposes an array of arrays (row/column swap); the outer array-of-arrays
//! shape is preserved.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_array_transpose(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 1,
            max_args: None,
            arg_kinds: vec![ParamKind::Array],
            return_kind: ReturnKind::SameAsArg(0),
        },
        args,
    )
}
