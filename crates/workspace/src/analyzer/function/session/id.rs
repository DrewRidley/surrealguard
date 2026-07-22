//! `session::id` function analysis: `session::id() -> option<string>`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ReturnKind, Signature};

pub(crate) fn analyze_session_id(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 0,
            max_args: Some(0),
            arg_kinds: vec![],
            return_kind: ReturnKind::Fixed(Kind::option(Kind::String)),
        },
        args,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::function::signature::evaluate;
    use crate::analyzer::function::signature::ParamKind;

    fn signature() -> Signature {
        Signature {
            min_args: 0,
            max_args: Some(0),
            arg_kinds: Vec::<ParamKind>::new(),
            return_kind: ReturnKind::Fixed(Kind::option(Kind::String)),
        }
    }

    #[test]
    fn returns_optional_string_for_zero_args() {
        assert_eq!(evaluate(&signature(), &[]), Kind::option(Kind::String));
    }
}
