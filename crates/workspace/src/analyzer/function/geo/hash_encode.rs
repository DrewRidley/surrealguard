//! `geo::hash::encode` function analysis: `geo::hash::encode(geometry, int?) -> string`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_geo_hash_encode(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 1,
            max_args: Some(2),
            arg_kinds: vec![
                ParamKind::Exact(Kind::Geometry(Vec::new())),
                ParamKind::Numeric,
            ],
            return_kind: ReturnKind::Fixed(Kind::String),
        },
        args,
    )
}
