//! Analysis fact types: what inference knows about one expression —
//! its kind, proven value, value class, and the partiality/dependency
//! trail. Produced by `analyzer::expression::infer`, consumed everywhere.

use serde::{Deserialize, Serialize};
use surrealdb_types::Kind;
use surrealguard_syntax::span::SourceSpan;

/// Why a piece of analysis is partial. Carried on transient facts and
/// emitted alongside spans at the inference site — never stored inside
/// response types: the type of something undeterminable is a plain
/// `Kind::Any` poison value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartialReason {
    Unresolved,
    DynamicExpression,
    UnsupportedSyntax(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpressionFact {
    pub span: SourceSpan,
    /// Best-effort inferred kind; `None` when undeterminable. Closed
    /// objects and tuples are expressed as `Kind::Literal`.
    pub kind: Option<Kind>,
    /// The expression's value when it is statically known: literals,
    /// arrays/objects of known values, and bindings tracing back to them.
    /// Lets value-dependent builtins (`type::field('name.first')`) resolve
    /// through `LET` indirection.
    pub value: Option<surrealdb_types::Value>,
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
            value: None,
            value_class,
            partial: Vec::new(),
            dependencies: ExpressionDependencies::default(),
        }
    }

    pub fn with_kind(mut self, kind: Kind) -> Self {
        self.kind = Some(kind);
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
    fn expression_fact_model_preserves_kind_class_and_partials() {
        let fact = ExpressionFact::new(span(), ExpressionValueClass::Literal)
            .with_kind(Kind::String)
            .with_partial(PartialReason::DynamicExpression);

        assert_eq!(fact.kind, Some(Kind::String));
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
