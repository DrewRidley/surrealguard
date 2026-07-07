//! `time::year` function analysis: `time::year(datetime) -> int`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_time_year(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Exact(Kind::Datetime)],
            return_kind: ReturnKind::Fixed(Kind::Int),
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
            arg_kinds: vec![ParamKind::Exact(Kind::Datetime)],
            return_kind: ReturnKind::Fixed(Kind::Int),
        }
    }

    #[test]
    fn returns_int_for_datetime_argument() {
        assert_eq!(evaluate(&signature(), &[Kind::Datetime]), Kind::Int);
    }
}
