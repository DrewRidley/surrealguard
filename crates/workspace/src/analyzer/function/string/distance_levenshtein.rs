//! `string::distance::levenshtein` function analysis: `string::distance::levenshtein(string, string) -> int`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 2,
        max_args: Some(2),
        arg_kinds: vec![
            ParamKind::Exact(Kind::String),
            ParamKind::Exact(Kind::String),
        ],
        return_kind: ReturnKind::Fixed(Kind::Int),
    }
}

pub(crate) fn analyze_string_distance_levenshtein(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
