//! `string::slice` function analysis: `string::slice(string, int, int) -> string`.
//!
//! Takes the source string, a start index, and an optional length; the length
//! argument may be omitted.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_string_slice(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 2,
            max_args: Some(3),
            arg_kinds: vec![
                ParamKind::Exact(Kind::String),
                ParamKind::Numeric,
                ParamKind::Numeric,
            ],
            return_kind: ReturnKind::Fixed(Kind::String),
        },
        args,
    )
}
