//! `string::semver::patch` function analysis: `string::semver::patch(string) -> int`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_string_semver_patch(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Exact(Kind::String)],
            return_kind: ReturnKind::Fixed(Kind::Int),
        },
        args,
    )
}
