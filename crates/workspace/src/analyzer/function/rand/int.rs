//! `rand::int` function analysis: `rand::int(min?, max?) -> int`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub fn analyze_rand_int(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 0,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Numeric],
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
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Numeric],
            return_kind: ReturnKind::Fixed(Kind::Int),
        }
    }

    #[test]
    fn returns_int_for_zero_args() {
        assert_eq!(evaluate(&signature(), &[]), Kind::Int);
    }

    #[test]
    fn returns_int_for_two_numeric_args() {
        assert_eq!(evaluate(&signature(), &[Kind::Int, Kind::Int]), Kind::Int);
    }
}
