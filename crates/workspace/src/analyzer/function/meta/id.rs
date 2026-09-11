//! `meta::id` function analysis: `meta::id(record) -> any`.
//!
//! Extracts the id portion of a record id. Record ids can be strings,
//! numbers, arrays, objects, or uuids, so the extracted id kind is
//! genuinely uncertain — an honest `any`, not a coverage gap. Only the
//! single-argument arity is checked.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(1),
        arg_kinds: vec![ParamKind::Any],
        return_kind: ReturnKind::Fixed(Kind::Any),
    }
}

pub(crate) fn analyze_meta_id(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
