//! `set::find` function analysis: `set::find(set, value) -> element`.
//!
//! Returns the first matching element (or NONE at runtime). The closure form
//! `set::find(set, |v| ...)` can't be expressed via kind-only signatures; with
//! a plain value argument the result is the set's element kind.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 2,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Array, ParamKind::Any],
        return_kind: ReturnKind::ArrayElement(0),
    }
}

pub(crate) fn analyze_set_find(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
