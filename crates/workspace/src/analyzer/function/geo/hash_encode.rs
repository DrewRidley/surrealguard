//! `geo::hash::encode` function analysis: `geo::hash::encode(geometry, int?) -> string`.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(2),
        arg_kinds: vec![
            ParamKind::Exact(Kind::Geometry(Vec::new())),
            ParamKind::Numeric,
        ],
        return_kind: ReturnKind::Fixed(Kind::String),
    }
}

pub(crate) fn analyze_geo_hash_encode(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
