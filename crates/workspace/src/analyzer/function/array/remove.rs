//! `array::remove` function analysis: `array::remove(array, index) -> array`.
//!
//! Removes the element at `index` (negative indices count from the end); the
//! array kind is preserved.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub fn analyze_array_remove(
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
            arg_kinds: vec![ParamKind::Array, ParamKind::Numeric],
            return_kind: ReturnKind::SameAsArg(0),
        },
        args,
    )
}
