//! `string::matches` function analysis:
//! `string::matches(string, string | regex) -> bool`.
//!
//! Tests the first argument against the second interpreted as a regular
//! expression — written either as a string or as a regex literal
//! (`string::matches('hello', /^h.*o$/)` is `true` on 3.2.3).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 2,
        max_args: Some(2),
        arg_kinds: vec![
            ParamKind::Exact(Kind::String),
            ParamKind::Exact(Kind::either(vec![Kind::String, Kind::Regex])),
        ],
        return_kind: ReturnKind::Fixed(Kind::Bool),
    }
}

pub(crate) fn analyze_string_matches(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
