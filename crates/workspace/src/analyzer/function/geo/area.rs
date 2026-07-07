//! `geo::area` function analysis: `geo::area(geometry) -> number`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_geo_area(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Exact(Kind::Geometry(Vec::new()))],
            return_kind: ReturnKind::Fixed(Kind::Number),
        },
        args,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signature() -> Signature {
        Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Exact(Kind::Geometry(Vec::new()))],
            return_kind: ReturnKind::Fixed(Kind::Number),
        }
    }

    #[test]
    fn area_of_geometry_is_number() {
        assert_eq!(
            evaluate(&signature(), &[Kind::Geometry(Vec::new())]),
            Kind::Number
        );
    }
}
