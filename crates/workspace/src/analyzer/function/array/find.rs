//! `array::find` function analysis: `array::find(array, value) -> element`.
//!
//! Returns the first matching element (or NONE at runtime). The closure form
//! `array::find(array, |v| ...)` can't be expressed via kind-only signatures;
//! with a plain value argument the result is the array's element kind.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_array_find(
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
            arg_kinds: vec![ParamKind::Array, ParamKind::Any],
            return_kind: ReturnKind::ArrayElement(0),
        },
        args,
    )
}
