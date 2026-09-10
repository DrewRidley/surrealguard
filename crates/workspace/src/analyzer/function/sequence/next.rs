//! `sequence::next` function analysis: `sequence::next(string) -> int`.
//!
//! Returns the next value of the named `DEFINE SEQUENCE`, which is integer.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(1),
        arg_kinds: vec![ParamKind::Exact(Kind::String)],
        return_kind: ReturnKind::Fixed(Kind::Int),
    }
}

pub(crate) fn analyze_sequence_next(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
