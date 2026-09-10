//! `object::remove` function analysis: `object::remove(object, string) -> object`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 2,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Object, ParamKind::Exact(Kind::String)],
        return_kind: ReturnKind::Fixed(Kind::Object),
    }
}

pub(crate) fn analyze_object_remove(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::function::signature::evaluate;

    #[test]
    fn remove_key_from_object_is_object() {
        assert_eq!(
            evaluate(&signature(), &[Kind::Object, Kind::String]),
            Kind::Object
        );
    }
}
