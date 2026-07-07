//! `search::analyze` function analysis:
//! `search::analyze(string, string) -> array<string>`.
//!
//! Runs the named analyzer over the given text and returns the produced
//! tokens as an array of strings.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub fn analyze_search_analyze(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![
                ParamKind::Exact(Kind::String),
                ParamKind::Exact(Kind::String),
            ],
            return_kind: ReturnKind::Fixed(Kind::Array(Box::new(Kind::String), None)),
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
                ParamKind::Exact(Kind::String),
                ParamKind::Exact(Kind::String),
            ],
            return_kind: ReturnKind::Fixed(Kind::Array(Box::new(Kind::String), None)),
        }
    }

    #[test]
    fn returns_string_array_for_two_strings() {
        assert_eq!(
            evaluate(&signature(), &[Kind::String, Kind::String]),
            Kind::Array(Box::new(Kind::String), None)
        );
    }
}
