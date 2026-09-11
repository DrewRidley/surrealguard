//! `rand::string` function analysis: `rand::string(length?, max?) -> string`.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 0,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Numeric],
        return_kind: ReturnKind::Fixed(Kind::String),
    }
}

pub(crate) fn analyze_rand_string(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
