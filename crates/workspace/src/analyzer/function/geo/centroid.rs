//! `geo::centroid` function analysis: `geo::centroid(geometry) -> geometry<point>`.

use surrealdb_types::{GeometryKind, Kind};

use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(1),
        arg_kinds: vec![ParamKind::Exact(Kind::Geometry(Vec::new()))],
        return_kind: ReturnKind::Fixed(Kind::Geometry(vec![GeometryKind::Point])),
    }
}

pub(crate) fn analyze_geo_centroid(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::function::signature::evaluate;

    #[test]
    fn centroid_of_geometry_is_point() {
        assert_eq!(
            evaluate(&signature(), &[Kind::Geometry(Vec::new())]),
            Kind::Geometry(vec![GeometryKind::Point])
        );
    }
}
