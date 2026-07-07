//! `object::from_entries` function analysis:
//! `object::from_entries(array) -> object`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_object_from_entries(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Array],
            return_kind: ReturnKind::Fixed(Kind::Object),
        },
        args,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signature() -> Signature {
        Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Array],
            return_kind: ReturnKind::Fixed(Kind::Object),
        }
    }

    #[test]
    fn from_entries_of_array_is_object() {
        assert_eq!(
            evaluate(&signature(), &[Kind::Array(Box::new(Kind::Any), None)]),
            Kind::Object
        );
    }
}
