//! `time::set_month` function analysis: `time::set_month(datetime, int) -> datetime`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_time_set_month(
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
            arg_kinds: vec![
                ParamKind::Exact(Kind::Datetime),
                ParamKind::Exact(Kind::Int),
            ],
            return_kind: ReturnKind::Fixed(Kind::Datetime),
        },
        args,
    )
}
