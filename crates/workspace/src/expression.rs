use serde::{Deserialize, Serialize};
use surrealdb_types::Kind;
use surrealguard_syntax::span::SourceSpan;

use crate::response_shape::{PartialReason, ResponseShape};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpressionFact {
    pub span: SourceSpan,
    pub kind: Option<Kind>,
    pub shape: Option<ResponseShape>,
    pub value_class: ExpressionValueClass,
    pub partial: Vec<PartialReason>,
    pub dependencies: ExpressionDependencies,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExpressionValueClass {
    Literal,
    FieldPath,
    Variable,
    FunctionCall,
    Subquery,
    Object,
    Array,
    GraphPath,
    Block,
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpressionDependencies {
    pub field_paths: Vec<String>,
    pub variables: Vec<String>,
    pub params: Vec<String>,
    pub function: Option<String>,
}

impl ExpressionFact {
    pub fn new(span: SourceSpan, value_class: ExpressionValueClass) -> Self {
        Self {
            span,
            kind: None,
            shape: None,
            value_class,
            partial: Vec::new(),
            dependencies: ExpressionDependencies::default(),
        }
    }

    pub fn with_kind(mut self, kind: Kind) -> Self {
        self.kind = Some(kind);
        self
    }

    pub fn with_shape(mut self, shape: ResponseShape) -> Self {
        self.shape = Some(shape);
        self
    }

    pub fn with_partial(mut self, reason: PartialReason) -> Self {
        self.partial.push(reason);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_syntax::source::SourceId;
    use surrealguard_syntax::span::ByteRange;

    fn span() -> SourceSpan {
        SourceSpan::new(
            SourceId::new("query:test"),
            ByteRange::new(4, 9).expect("test range should be valid"),
        )
    }

    #[test]
    fn expression_fact_model_preserves_kind_shape_class_and_partials() {
        let fact = ExpressionFact::new(span(), ExpressionValueClass::Literal)
            .with_kind(Kind::String)
            .with_shape(ResponseShape::Value { kind: Kind::String })
            .with_partial(PartialReason::DynamicExpression);

        assert_eq!(fact.kind, Some(Kind::String));
        assert_eq!(
            fact.shape,
            Some(ResponseShape::Value { kind: Kind::String })
        );
        assert_eq!(fact.value_class, ExpressionValueClass::Literal);
        assert_eq!(fact.partial, vec![PartialReason::DynamicExpression]);
    }

    #[test]
    fn expression_fact_model_tracks_dependencies_without_new_type_system() {
        let mut fact =
            ExpressionFact::new(span(), ExpressionValueClass::FunctionCall).with_kind(Kind::Int);
        fact.dependencies.function = Some("string::len".into());
        fact.dependencies.field_paths.push("profile.name".into());
        fact.dependencies.params.push("fallback".into());
        fact.dependencies.variables.push("display_name".into());

        assert_eq!(fact.kind, Some(Kind::Int));
        assert_eq!(fact.dependencies.function.as_deref(), Some("string::len"));
        assert_eq!(fact.dependencies.field_paths, vec!["profile.name"]);
        assert_eq!(fact.dependencies.params, vec!["fallback"]);
        assert_eq!(fact.dependencies.variables, vec!["display_name"]);
    }
}
