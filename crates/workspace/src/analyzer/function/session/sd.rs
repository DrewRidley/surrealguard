//! `session::sd` function analysis: `session::sd() -> option<record>`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 0,
        max_args: Some(0),
        arg_kinds: vec![],
        return_kind: ReturnKind::Fixed(Kind::option(Kind::Record(Vec::new()))),
    }
}

pub(crate) fn analyze_session_sd(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
