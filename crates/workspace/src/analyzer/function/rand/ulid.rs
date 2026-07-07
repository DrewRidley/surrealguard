//! `rand::ulid` function analysis: `rand::ulid(datetime?) -> string`.
//!
//! There is no ULID-specific `Kind`, so a ULID is modelled as `String`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_rand_ulid(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 0,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Any],
            return_kind: ReturnKind::Fixed(Kind::String),
        },
        args,
    )
}
