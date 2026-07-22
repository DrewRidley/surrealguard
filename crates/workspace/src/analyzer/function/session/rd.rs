//! `session::rd` function analysis: `session::rd() -> option<record>`.
//!
//! Returns the record the current session authenticated as (record access),
//! or `NONE` when the session is not authenticated as a record.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ReturnKind, Signature};

pub(crate) fn analyze_session_rd(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 0,
            max_args: Some(0),
            arg_kinds: vec![],
            return_kind: ReturnKind::Fixed(Kind::option(Kind::Record(Vec::new()))),
        },
        args,
    )
}
