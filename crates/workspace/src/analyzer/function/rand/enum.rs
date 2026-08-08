//! `rand::enum` function analysis: `rand::enum(a, b, ...) -> a | b | ...`.
//!
//! Returns one of its arguments, so the result type is their union.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

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
    use surrealguard_diagnostics::Finding;
    use surrealguard_syntax::source::SourceId;

    use crate::schema::SchemaIndex;

    #[test]
    fn returns_the_union_of_its_arguments() {
        let schema = SchemaIndex::default();
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(&schema, SourceId::new("fn:test"), "", &mut diagnostics);
        let call = ast::Call {
            path: ast::Spanned::new(
                "rand::enum".into(),
                surrealguard_syntax::span::ByteRange::new(0, 1).unwrap(),
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
