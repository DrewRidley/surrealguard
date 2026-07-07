//! `object::keys` function analysis: `object::keys(object) -> array<string>`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub fn analyze_object_keys(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Object],
            return_kind: ReturnKind::Fixed(Kind::Array(Box::new(Kind::String), None)),
        },
        args,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::function::signature::evaluate;

    fn signature() -> Signature {
        Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Object],
            return_kind: ReturnKind::Fixed(Kind::Array(Box::new(Kind::String), None)),
        }
    }

    #[test]
    fn keys_of_object_is_array_of_strings() {
        assert_eq!(
            evaluate(&signature(), &[Kind::Object]),
            Kind::Array(Box::new(Kind::String), None)
        );
    }
}
