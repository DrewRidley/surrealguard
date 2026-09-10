//! `time::set_hour` function analysis: `time::set_hour(datetime, int) -> datetime`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 2,
        max_args: Some(2),
        arg_kinds: vec![
            ParamKind::Exact(Kind::Datetime),
            ParamKind::Exact(Kind::Int),
        ],
        return_kind: ReturnKind::Fixed(Kind::Datetime),
    }
}

pub(crate) fn analyze_time_set_hour(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
