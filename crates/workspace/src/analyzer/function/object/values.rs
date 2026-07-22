//! `object::values` function analysis: `object::values(object) -> array`.
//!
//! When the argument's kind is a closed object literal, the element kind
//! is the union of the field kinds; a bare `object` yields `array<any>`.

use surrealdb_types::{Kind, KindLiteral};
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub(crate) fn analyze_object_values(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    match args {
        [Kind::Literal(KindLiteral::Object(fields))] => {
            let element = if fields.is_empty() {
                Kind::Any
            } else {
                Kind::either(fields.values().cloned().collect())
            };
            Kind::Array(Box::new(element), Some(fields.len() as u64))
        }
        [Kind::Object] => Kind::Array(Box::new(Kind::Any), None),
        _ => Kind::Any,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use surrealguard_diagnostics::Finding;
    use surrealguard_syntax::source::SourceId;

    use crate::schema::SchemaIndex;

    #[test]
    fn unions_the_field_kinds_of_literal_objects() {
        let schema = SchemaIndex::default();
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(&schema, SourceId::new("fn:test"), "", &mut diagnostics);
        let call = ast::Call {
            path: ast::Spanned::new(
                "object::values".into(),
                surrealguard_syntax::span::ByteRange::new(0, 1).unwrap(),
            ),
            args: Vec::new(),
        };
        let mut fields = BTreeMap::new();
        fields.insert("a".to_string(), Kind::Int);
        fields.insert("b".to_string(), Kind::String);

        let kind = analyze_object_values(
            &mut ctx,
            &call,
            &[Kind::Literal(KindLiteral::Object(fields))],
        );

        assert_eq!(
            kind,
            Kind::Array(
                Box::new(Kind::Either(vec![Kind::Int, Kind::String])),
                Some(2)
            )
        );
    }
}
