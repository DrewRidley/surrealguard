//! `array::union` function analysis: `array::union(array, array, ...) -> array`.
//!
//! Merges two or more arrays keeping unique values; the first array's kind is
//! preserved.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 2,
        max_args: None,
        arg_kinds: vec![ParamKind::Array],
        return_kind: ReturnKind::SameAsArg(0),
    }
}

pub(crate) fn analyze_array_union(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
