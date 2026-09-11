//! `rand::enum` function analysis: `rand::enum(a, b, ...) -> a | b | ...`.
//!
//! Returns one of its arguments, so the result type is their union.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{ParamKind, ReturnKind, Signature};

/// The declared shape, for the catalog; the analyzer body below derives
/// the return kind itself rather than checking calls against this.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: None,
        arg_kinds: vec![ParamKind::Any],
        return_kind: ReturnKind::Fixed(Kind::Any),
    }
}

pub(crate) fn analyze_rand_enum(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    if args.is_empty() {
        return Kind::Any;
    }
    Kind::either(args.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealql_analyzer_diagnostics::Finding;
    use surrealql_analyzer_syntax::source::SourceId;

    use crate::schema::SchemaIndex;

    #[test]
    fn returns_the_union_of_its_arguments() {
        let schema = SchemaIndex::default();
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(&schema, SourceId::new("fn:test"), "", &mut diagnostics);
        let call = ast::Call {
            path: ast::Spanned::new(
                "rand::enum".into(),
                surrealql_analyzer_syntax::span::ByteRange::new(0, 1).unwrap(),
            ),
            written: "rand::enum".into(),
            args: Vec::new(),
        };

        assert_eq!(
            analyze_rand_enum(&mut ctx, &call, &[Kind::Int, Kind::String]),
            Kind::Either(vec![Kind::Int, Kind::String])
        );
        assert_eq!(
            analyze_rand_enum(&mut ctx, &call, &[Kind::Int, Kind::Int]),
            Kind::Int
        );
    }
}
