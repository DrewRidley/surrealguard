//! `vector::distance::minkowski` function analysis: `vector::distance::minkowski(array, array, number) -> number`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 3,
        max_args: Some(3),
        arg_kinds: vec![ParamKind::Array, ParamKind::Array, ParamKind::Numeric],
        return_kind: ReturnKind::Fixed(Kind::Number),
    }
}

pub(crate) fn analyze_vector_distance_minkowski(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
