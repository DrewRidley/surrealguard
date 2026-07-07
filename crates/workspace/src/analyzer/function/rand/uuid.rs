//! `rand::uuid` function analysis: `rand::uuid() -> uuid`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ReturnKind, Signature};

pub fn analyze_rand_uuid(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 0,
            max_args: Some(0),
            arg_kinds: vec![],
            return_kind: ReturnKind::Fixed(Kind::Uuid),
        },
        args,
    )
}
