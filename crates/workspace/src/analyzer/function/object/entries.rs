//! `object::entries` function analysis:
//! `object::entries(object) -> array<array<any>>` (an array of `[key, value]` pairs).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_object_entries(
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
            arg_kinds: vec![ParamKind::Object],
            return_kind: ReturnKind::Fixed(Kind::Array(
                Box::new(Kind::Array(Box::new(Kind::Any), None)),
                None,
            )),
        },
        args,
    )
}
