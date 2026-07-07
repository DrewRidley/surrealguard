//! `type::record` function analysis: `type::record(any) -> record`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_type_record(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 1,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Any],
            return_kind: ReturnKind::Fixed(Kind::Record(Vec::new())),
        },
        args,
    )
}
