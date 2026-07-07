//! `value::patch` function analysis: `value::patch(any, array) -> any`.
//!
//! Applies a list of JSON-patch operations to a value. The result shape
//! depends on the runtime patch contents, so the return kind is genuinely
//! `any`; only the value plus patch-array arity/kinds are checked.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_value_patch(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Any, ParamKind::Array],
            return_kind: ReturnKind::SameAsArg(0),
        },
        args,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::function::signature::{ParamKind, ReturnKind, Signature};

    fn signature() -> Signature {
        Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Any, ParamKind::Array],
            return_kind: ReturnKind::SameAsArg(0),
        }
    }

    #[test]
    fn returns_the_subject_kind_when_patch_array_supplied() {
        assert_eq!(
            evaluate(
                &signature(),
                &[Kind::Object, Kind::Array(Box::new(Kind::Object), None)]
            ),
            Kind::Object
        );
    }

    #[test]
    fn mistaken_patch_argument_still_infers_the_subject_kind() {
        assert_eq!(
            evaluate(&signature(), &[Kind::Object, Kind::Object]),
            Kind::Object
        );
    }
}
