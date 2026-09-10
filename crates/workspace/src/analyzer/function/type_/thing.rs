//! `type::thing` function analysis: `type::thing(table, id) -> record`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Any],
        return_kind: ReturnKind::Fixed(Kind::Record(Vec::new())),
    }
}

pub(crate) fn analyze_type_thing(
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
    fn constructs_unconstrained_record() {
        assert_eq!(
            evaluate(&signature(), &[Kind::String, Kind::Int]),
            Kind::Record(Vec::new())
        );
    }
}
