//! `value::expect` function analysis: `value::expect(any, string) -> any`.
//!
//! Runtime type assertion: checks a value against a kind named by a string
//! and errors otherwise. The narrowed kind is encoded in a runtime string
//! argument that static analysis can't resolve, so the result is a
//! legitimately honest `any`. Only the value plus kind-string arity/kinds are
//! checked.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_value_expect(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Any, ParamKind::Exact(Kind::String)],
            return_kind: ReturnKind::SameAsArg(0),
        },
        args,
    )
}
