//! `set::slice` function analysis: `set::slice(set, start, len) -> set`.
//!
//! `start` and `len` are optional; the result preserves the input set's kind.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub fn analyze_set_slice(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 1,
            max_args: Some(3),
            arg_kinds: vec![ParamKind::Array, ParamKind::Numeric, ParamKind::Numeric],
            return_kind: ReturnKind::SameAsArg(0),
        },
        args,
    )
}
