//! `string::contains` function analysis: `string::contains(string, string) -> bool`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_string_contains(
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
                ParamKind::Exact(Kind::String),
                ParamKind::Exact(Kind::String),
            ],
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
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![
                ParamKind::Exact(Kind::String),
                ParamKind::Exact(Kind::String),
            ],
            return_kind: ReturnKind::Fixed(Kind::Bool),
        }
    }

    #[test]
    fn returns_bool_for_two_strings() {
        assert_eq!(
            evaluate(&signature(), &[Kind::String, Kind::String]),
            Kind::Bool
        );
    }
}
