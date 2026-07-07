//! `not` function analysis: `not(value) -> bool`.
//!
//! Function form of the `!` operator. SurrealDB inverts the truthiness of
//! any value, so the argument is unconstrained (`Any`) and the result is
//! always `bool`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_not_not(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Any],
            return_kind: ReturnKind::Fixed(Kind::Bool),
        },
        args,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signature() -> Signature {
        Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Any],
            return_kind: ReturnKind::Fixed(Kind::Bool),
        }
    }

    #[test]
    fn returns_bool_for_any_value() {
        assert_eq!(evaluate(&signature(), &[Kind::Int]), Kind::Bool);
    }
}
