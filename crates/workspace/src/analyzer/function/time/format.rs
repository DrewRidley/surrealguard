//! `time::format` function analysis: `time::format(datetime, string) -> string`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub fn analyze_time_format(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![
                ParamKind::Exact(Kind::Datetime),
                ParamKind::Exact(Kind::String),
            ],
            return_kind: ReturnKind::Fixed(Kind::String),
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
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![
                ParamKind::Exact(Kind::Datetime),
                ParamKind::Exact(Kind::String),
            ],
            return_kind: ReturnKind::Fixed(Kind::String),
        }
    }

    #[test]
    fn returns_string_for_valid_call() {
        assert_eq!(
            evaluate(&signature(), &[Kind::Datetime, Kind::String]),
            Kind::String
        );
    }
}
