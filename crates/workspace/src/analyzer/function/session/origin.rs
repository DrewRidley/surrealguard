//! `session::origin` function analysis: `session::origin() -> option<string>`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ReturnKind, Signature};

pub fn analyze_session_origin(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 0,
            max_args: Some(0),
            arg_kinds: vec![],
            return_kind: ReturnKind::Fixed(Kind::option(Kind::String)),
        },
        args,
    )
}
