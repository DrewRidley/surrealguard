//! `file::head` function analysis: `file::head(file) -> object`.
//!
//! Returns file metadata (size, content type, etc.) as an object; the
//! individual field set is runtime-dependent so only `object` is inferred.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(1),
        arg_kinds: vec![ParamKind::Any],
        return_kind: ReturnKind::Fixed(Kind::Object),
    }
}

pub(crate) fn analyze_file_head(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    // File-pointer argument left `Any`; see `file::bucket` for why.
    evaluate(&signature(), args)
}
