//! `search::highlight` function analysis:
//! `search::highlight(string, string, number [, bool]) -> string`.
//!
//! Wraps the matched terms of a search predicate with the given prefix/suffix
//! markers. The trailing optional `bool` selects whole-term highlighting.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 3,
        max_args: Some(4),
        arg_kinds: vec![
            ParamKind::Exact(Kind::String),
            ParamKind::Exact(Kind::String),
            ParamKind::Numeric,
            ParamKind::Exact(Kind::Bool),
        ],
        return_kind: ReturnKind::Fixed(Kind::String),
    }
}

pub(crate) fn analyze_search_highlight(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
