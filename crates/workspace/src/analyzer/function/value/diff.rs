//! `value::diff` function analysis: `value::diff(any, any) -> array<object>`.
//!
//! Computes the JSON-patch difference between two values, yielding a list of
//! patch operation objects.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 2,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Any, ParamKind::Any],
        return_kind: ReturnKind::Fixed(Kind::Array(Box::new(Kind::Object), None)),
    }
}

pub(crate) fn analyze_value_diff(
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
    fn returns_array_of_objects() {
        assert_eq!(
            evaluate(&signature(), &[Kind::Object, Kind::Object]),
            Kind::Array(Box::new(Kind::Object), None)
        );
    }
}
