//! `array::sequence` function analysis:
//! `array::sequence(offset, len?) -> array<int>`.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Numeric, ParamKind::Numeric],
        return_kind: ReturnKind::Fixed(Kind::Array(Box::new(Kind::Int), None)),
    }
}

pub(crate) fn analyze_array_sequence(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
