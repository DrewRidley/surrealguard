//! `count` function analysis: `count()` / `count(value) -> int`.
//!
//! The zero-arg form (`SELECT count() FROM ...`) counts rows; the one-arg
//! form counts truthy values / array length. Both always yield `int`, so the
//! optional argument is left unconstrained (`Any`) — it only affects arity.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub fn analyze_count_count(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 0,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Any],
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
            min_args: 0,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Any],
            return_kind: ReturnKind::Fixed(Kind::Int),
        }
    }

    #[test]
    fn zero_arg_form_returns_int() {
        assert_eq!(evaluate(&signature(), &[]), Kind::Int);
    }

    #[test]
    fn one_arg_form_returns_int() {
        assert_eq!(evaluate(&signature(), &[Kind::Bool]), Kind::Int);
    }
}
