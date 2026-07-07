//! `rand::duration` function analysis: `rand::duration(min?, max?) -> duration`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_rand_duration(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 0,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Numeric],
            return_kind: ReturnKind::Fixed(Kind::Duration),
        },
        args,
    )
}
