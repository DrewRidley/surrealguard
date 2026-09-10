//! `sleep` function analysis: `sleep(duration) -> none`.
//!
//! A side-effecting no-op that pauses execution for the given duration and
//! produces no meaningful value, so its return kind is `none`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(1),
        arg_kinds: vec![ParamKind::Exact(Kind::Duration)],
        return_kind: ReturnKind::Fixed(Kind::None),
    }
}

pub(crate) fn analyze_sleep_sleep(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::function::signature::evaluate;

    #[test]
    fn returns_none_for_duration() {
        assert_eq!(evaluate(&signature(), &[Kind::Duration]), Kind::None);
    }
}
