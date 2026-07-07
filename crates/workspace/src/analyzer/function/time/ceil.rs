//! `time::ceil` function analysis: `time::ceil(datetime, duration) -> datetime`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_time_ceil(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![
                ParamKind::Exact(Kind::Datetime),
                ParamKind::Exact(Kind::Duration),
            ],
            return_kind: ReturnKind::Fixed(Kind::Datetime),
        },
        args,
    )
}
