//! `type::point` function analysis: `type::point(number, number) -> geometry<point>`.

use surrealdb_types::{GeometryKind, Kind};

use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Any],
        return_kind: ReturnKind::Fixed(Kind::Geometry(vec![GeometryKind::Point])),
    }
}

pub(crate) fn analyze_type_point(
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
    fn constructs_geometry_point() {
        assert_eq!(
            evaluate(&signature(), &[Kind::Float, Kind::Float]),
            Kind::Geometry(vec![GeometryKind::Point])
        );
    }
}
