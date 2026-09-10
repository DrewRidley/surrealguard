//! `string::concat` function analysis: `string::concat(...any) -> string`.
//!
//! Variadic: concatenates any number of arguments (of any kind, coerced to
//! their string form) into a single string.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: None,
        arg_kinds: vec![ParamKind::Any],
        return_kind: ReturnKind::Fixed(Kind::String),
    }
}

pub(crate) fn analyze_string_concat(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
