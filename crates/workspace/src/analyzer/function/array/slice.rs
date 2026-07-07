//! `array::slice` function analysis: `array::slice(array, [start], [len]) -> array`.
//!
//! `start` and `len` are optional; the result preserves the input array's
//! kind.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_array_slice(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 1,
            max_args: Some(3),
            arg_kinds: vec![ParamKind::Array, ParamKind::Numeric, ParamKind::Numeric],
            return_kind: ReturnKind::SameAsArg(0),
        },
        args,
    )
}
