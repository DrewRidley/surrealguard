//! `string::repeat` function analysis: `string::repeat(string, int) -> string`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_string_repeat(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Exact(Kind::String), ParamKind::Numeric],
            return_kind: ReturnKind::Fixed(Kind::String),
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
            arg_kinds: vec![ParamKind::Exact(Kind::String), ParamKind::Numeric],
            return_kind: ReturnKind::Fixed(Kind::String),
        }
    }

    #[test]
    fn returns_string_for_string_and_count() {
        assert_eq!(
            evaluate(&signature(), &[Kind::String, Kind::Int]),
            Kind::String
        );
    }
}
