//! `crypto::joaat` function analysis: `crypto::joaat(string) -> string`.
//!
//! `crypto::joaat` is a 32-bit Jenkins one-at-a-time hash, but SurrealDB
//! returns it as its hexadecimal string representation (like the other
//! `crypto::*` digests), not as an integer.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(1),
        arg_kinds: vec![ParamKind::Exact(Kind::String)],
        return_kind: ReturnKind::Fixed(Kind::String),
    }
}

pub(crate) fn analyze_crypto_joaat(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
