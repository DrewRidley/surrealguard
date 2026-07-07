//! `rand::bool` function analysis: `rand::bool() -> bool`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ReturnKind, Signature};

pub fn analyze_rand_bool(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 0,
            max_args: Some(0),
            arg_kinds: vec![],
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
            min_args: 0,
            max_args: Some(0),
            arg_kinds: vec![],
            return_kind: ReturnKind::Fixed(Kind::Bool),
        }
    }

    #[test]
    fn returns_bool_for_zero_args() {
        assert_eq!(evaluate(&signature(), &[]), Kind::Bool);
    }
}
