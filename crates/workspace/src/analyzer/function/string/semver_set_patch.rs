//! `string::semver::set::patch` function analysis: `string::semver::set::patch(string, int) -> string`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_string_semver_set_patch(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Exact(Kind::String), ParamKind::Numeric],
            return_kind: ReturnKind::Fixed(Kind::String),
        },
        args,
    )
}
