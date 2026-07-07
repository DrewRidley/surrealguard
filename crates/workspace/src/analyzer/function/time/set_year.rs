//! `time::set_year` function analysis: `time::set_year(datetime, int) -> datetime`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_time_set_year(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![
                ParamKind::Exact(Kind::Datetime),
                ParamKind::Exact(Kind::Int),
            ],
            return_kind: ReturnKind::Fixed(Kind::Datetime),
        },
        args,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signature() -> Signature {
        Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![
                ParamKind::Exact(Kind::Datetime),
                ParamKind::Exact(Kind::Int),
            ],
            return_kind: ReturnKind::Fixed(Kind::Datetime),
        }
    }

    #[test]
    fn returns_datetime_for_valid_call() {
        assert_eq!(
            evaluate(&signature(), &[Kind::Datetime, Kind::Int]),
            Kind::Datetime
        );
    }
}
