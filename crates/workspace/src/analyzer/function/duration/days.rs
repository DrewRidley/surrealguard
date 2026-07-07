//! `duration::days` function analysis: `duration::days(duration) -> int`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub fn analyze_duration_days(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Exact(Kind::Duration)],
            return_kind: ReturnKind::Fixed(Kind::Int),
        },
        args,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::function::signature::evaluate;

    fn signature() -> Signature {
        Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Exact(Kind::Duration)],
            return_kind: ReturnKind::Fixed(Kind::Int),
        }
    }

    #[test]
    fn decomposes_duration_to_int() {
        assert_eq!(evaluate(&signature(), &[Kind::Duration]), Kind::Int);
    }
}
