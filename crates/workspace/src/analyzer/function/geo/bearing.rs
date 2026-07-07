//! `geo::bearing` function analysis: `geo::bearing(geometry, geometry) -> number`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_geo_bearing(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![
                ParamKind::Exact(Kind::Geometry(Vec::new())),
                ParamKind::Exact(Kind::Geometry(Vec::new())),
            ],
            return_kind: ReturnKind::Fixed(Kind::Number),
        },
        args,
    )
}
