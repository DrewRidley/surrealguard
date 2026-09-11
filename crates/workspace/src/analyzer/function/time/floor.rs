//! `time::floor` function analysis: `time::floor(datetime, duration) -> datetime`.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 2,
        max_args: Some(2),
        arg_kinds: vec![
            ParamKind::Exact(Kind::Datetime),
            ParamKind::Exact(Kind::Duration),
        ],
        return_kind: ReturnKind::Fixed(Kind::Datetime),
    }
}

pub(crate) fn analyze_time_floor(
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
    fn returns_datetime_for_valid_call() {
        assert_eq!(
            evaluate(&signature(), &[Kind::Datetime, Kind::Duration]),
            Kind::Datetime
        );
    }
}
