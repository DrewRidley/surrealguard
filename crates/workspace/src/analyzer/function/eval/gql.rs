//! `eval::gql` function analysis:
//! `eval::gql(query: string, bindings?: object) -> any`.
//!
//! The nested query is a runtime string, so its result shape is genuinely
//! unknowable statically — only the arguments are checked.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_eval_gql(
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
            arg_kinds: vec![ParamKind::Exact(Kind::String), ParamKind::Object],
            return_kind: ReturnKind::Fixed(Kind::Any),
        },
        args,
    )
}
