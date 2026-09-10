//! `record::refs` function analysis: `record::refs(record, ...) -> array<any>`.
//!
//! Returns the records that reference the given record, optionally filtered by
//! table and field. The referencing records can span tables, so the element
//! kind isn't determinable statically — an `array<any>` is the honest result.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(3),
        arg_kinds: vec![ParamKind::Any],
        return_kind: ReturnKind::Fixed(Kind::Array(Box::new(Kind::Any), None)),
    }
}

pub(crate) fn analyze_record_refs(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
