//! `array::first` function analysis: `array::first(array) -> element`.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(1),
        arg_kinds: vec![ParamKind::Array],
        return_kind: ReturnKind::ArrayElement(0),
    }
}

pub(crate) fn analyze_array_first(
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
    fn returns_the_array_element_kind() {
        assert_eq!(
            evaluate(
                &signature(),
                &[Kind::Array(Box::new(Kind::String), Some(3))]
            ),
            Kind::String
        );
    }

    #[test]
    fn rejects_wrong_arity() {
        assert_eq!(evaluate(&signature(), &[]), Kind::Any);
    }
}
