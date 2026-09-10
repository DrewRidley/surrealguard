//! `rand::uuid::v4` function analysis: `rand::uuid::v4() -> uuid`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 0,
        max_args: Some(0),
        arg_kinds: vec![],
        return_kind: ReturnKind::Fixed(Kind::Uuid),
    }
}

pub(crate) fn analyze_rand_uuid_v4(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
