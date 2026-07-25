//! `rand::uuid::v7` function analysis: `rand::uuid::v7(datetime?) -> uuid`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_rand_uuid_v7(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 0,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Exact(Kind::Datetime)],
            return_kind: ReturnKind::Fixed(Kind::Uuid),
        },
        args,
    )
}
