//! `time::group` function analysis: `time::group(datetime, string) -> datetime`.
//!
//! The second argument is a grouping unit string (e.g. `'hour'`, `'day'`,
//! `'week'`, `'month'`, `'year'`), so it is checked as `Kind::String`.

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
            ParamKind::Exact(Kind::Datetime),
            ParamKind::Exact(Kind::String),
        ],
        return_kind: ReturnKind::Fixed(Kind::Datetime),
    }
}

pub(crate) fn analyze_time_group(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}
