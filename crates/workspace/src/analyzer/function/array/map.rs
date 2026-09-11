//! `array::map` function analysis: `array::map(array, closure) -> array`.
//!
//! The element kind of the result is the closure's return kind, inferred
//! with the closure's parameters bound to the call site's element kind (the
//! closure receives `[value, index]`). Mapping preserves the array length.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::expression::infer::closure_return_kind;
use crate::analyzer::function::signature::{ParamKind, ReturnKind, Signature};
use crate::analyzer::function::{check_closure_arity, closure_arg};

/// The declared shape, for the catalog; the analyzer body below derives
/// the return kind itself rather than checking calls against this.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 2,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Array, ParamKind::Closure],
        return_kind: ReturnKind::Fixed(Kind::Array(Box::new(Kind::Any), None)),
    }
}

pub(crate) fn analyze_array_map(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let Some((Kind::Array(element, max_len), closure)) =
        args.first().cloned().zip(closure_arg(call, 1))
    else {
        return Kind::Any;
    };
    check_closure_arity(ctx, call, closure, 2);

    let mapped =
        closure_return_kind(closure, &[(*element).clone(), Kind::Int], ctx).unwrap_or(Kind::Any);
    Kind::Array(Box::new(mapped), max_len)
}

#[cfg(test)]
mod tests {
    use surrealdb_types::Kind;
    use surrealql_analyzer_diagnostics::Finding;
    use surrealql_analyzer_syntax::parse::parse_source;
    use surrealql_analyzer_syntax::source::SourceId;

    use crate::analyzer::context::AnalysisContext;
    use crate::schema::SchemaIndex;

    fn analyze(query: &str) -> Kind {
        let parsed = parse_source(SourceId::new("fn:test"), query).expect("query parses");
        let lowered = surrealql_analyzer_syntax::lower::lower_first_expr(&parsed, "FunctionCall")
            .expect("function call node");
        let schema = SchemaIndex::default();
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(
            &schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
        );
        crate::analyzer::expression::analyze_expr(&mut ctx, &lowered)
    }

    #[test]
    fn maps_the_element_kind_through_the_closure_body() {
        // The closure's parameter is bound to the element kind at the call
        // site: int elements through `* 2` stay int, and the length is
        // preserved.
        assert_eq!(
            analyze("RETURN array::map([1, 2], |$v| $v * 2);"),
            Kind::Array(Box::new(Kind::Int), Some(2))
        );
        // A body that changes the kind changes the element kind.
        assert_eq!(
            analyze("RETURN array::map([1, 2], |$v| -> string { RETURN 'x'; });"),
            Kind::Array(Box::new(Kind::String), Some(2))
        );
    }
}
