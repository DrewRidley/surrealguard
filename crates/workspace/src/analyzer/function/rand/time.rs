//! `rand::time` function analysis: `rand::time(min?, max?) -> datetime`.
//!
//! Optional `min`/`max` bounds are unix timestamps (numeric).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub fn analyze_rand_time(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 0,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Numeric],
            return_kind: ReturnKind::Fixed(Kind::Datetime),
        },
        args,
    )
}
