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
    /// A referenced binding, field, or definition could not be resolved.
    Unresolved,
    /// The value is only known at runtime, so it cannot be inferred.
    DynamicExpression,
    /// Syntax the inference engine does not model, carrying a description.
    UnsupportedSyntax(String),
}

/// What inference proved about a single expression.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpressionFact {
    /// Where the expression sits in its source.
    pub span: SourceSpan,
    /// Best-effort inferred kind; `None` when undeterminable. Closed
    /// objects and tuples are expressed as `Kind::Literal`.
    pub kind: Option<Kind>,
    /// The expression's value when it is statically known: literals,
    /// arrays/objects of known values, and bindings tracing back to them.
    /// Lets value-dependent builtins (`type::field('name.first')`) resolve
    /// through `LET` indirection.
    pub value: Option<surrealdb_types::Value>,
    /// The syntactic shape the value came from — what kind of expression
    /// this fact describes.
    pub value_class: ExpressionValueClass,
    /// Every reason the fact is incomplete; empty when fully determined.
    pub partial: Vec<PartialReason>,
    /// The fields, variables, params, and function this expression reads.
    pub dependencies: ExpressionDependencies,
}

/// The syntactic category of the expression a fact describes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExpressionValueClass {
    /// A literal constant.
    Literal,
    /// A field access idiom (`profile.name`).
    FieldPath,
    /// A `LET` binding reference.
    Variable,
    /// A function or method call.
    FunctionCall,
    /// A parenthesized subquery.
    Subquery,
    /// An object constructor.
    Object,
    /// An array constructor.
    Array,
    /// A graph traversal (`->edge->node`).
    GraphPath,
    /// A `{ ... }` statement block.
    Block,
    /// A shape inference does not classify.
    Unknown,
}

/// The named symbols an expression depends on, tracked so downstream
/// consumers can trace values through `LET` and param indirection.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpressionDependencies {
    /// Field paths read by the expression.
    pub field_paths: Vec<String>,
    /// `LET` variable names read by the expression.
    pub variables: Vec<String>,
    /// Parameter names (`$name`) read by the expression.
    pub params: Vec<String>,
    /// The function called, when the expression is a call.
    pub function: Option<String>,
}

impl ExpressionFact {
    /// A fresh fact for `span` with the given class and nothing yet known.
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

    /// Sets the inferred kind, consuming and returning the fact.
    pub fn with_kind(mut self, kind: Kind) -> Self {
        self.kind = Some(kind);
        self
    }

    /// Records one reason the fact is partial, consuming and returning it.
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
