//! `record::id` function analysis: `record::id(record) -> any`.
//!
//! A record ID can be one of many shapes (string, int, uuid, array, object,
//! range, ...), so the ID kind is genuinely not determinable statically —
//! `Any` here is an honest "unknown", not an unimplemented gap.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_record_id(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Any],
            return_kind: ReturnKind::Fixed(Kind::Any),
        },
        args,
    )
}
