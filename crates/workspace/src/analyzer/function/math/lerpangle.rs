//! `math::lerpangle` function analysis: `math::lerpangle(number, number, fraction) -> number`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub fn analyze_math_lerpangle(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 3,
            max_args: Some(3),
            arg_kinds: vec![ParamKind::Numeric, ParamKind::Numeric, ParamKind::Numeric],
            return_kind: ReturnKind::Fixed(Kind::Number),
        },
        args,
    )
}
