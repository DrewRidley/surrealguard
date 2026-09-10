//! `rand::duration` function analysis: `rand::duration(min?, max?) -> duration`.
//!
//! Upstream's bounds are **durations**, not numbers
//! (`fn duration((dur1, dur2): (Duration, Duration))`), so
//! `rand::duration(1s, 2s)` is the valid call and `rand::duration(1, 2)` is
//! not. The arity stays lenient (upstream requires both bounds in 3.x, but
//! the range is optional in the documented surface) — a missed arity
//! violation costs nothing next to rejecting a valid call.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 0,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Exact(Kind::Duration)],
        return_kind: ReturnKind::Fixed(Kind::Duration),
    }
}

pub(crate) fn analyze_rand_duration(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
