//! `file::get` function analysis: `file::get(file) -> bytes`.
//!
//! Returns the raw contents of the file as `bytes`; the byte payload is
//! runtime data but its kind is statically known.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_file_get(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    // File-pointer argument left `Any`; see `file::bucket` for why.
    evaluate(
        &Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Any],
            return_kind: ReturnKind::Fixed(Kind::Bytes),
        },
        args,
    )
}
