use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use surrealdb_types::Kind;
use surrealguard_syntax::parse::ParsedSource;
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};
use tree_sitter::Node;

use crate::response_shape::{FieldShape, PartialReason, ResponseShape};
use crate::schema::TableDef;

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

pub fn infer_expression_fact(
    node: Node<'_>,
    parsed: &ParsedSource,
    row_table: Option<&TableDef>,
) -> ExpressionFact {
    match node.kind() {
        "Predicate" | "Fields" => infer_single_child_expression_fact(node, parsed, row_table),
        "String" => ExpressionFact::new(
            node_span(node, parsed.source_id().clone()),
            ExpressionValueClass::Literal,
        )
        .with_kind(Kind::String)
        .with_shape(ResponseShape::Value { kind: Kind::String }),
        "Bool" => ExpressionFact::new(
            node_span(node, parsed.source_id().clone()),
            ExpressionValueClass::Literal,
        )
        .with_kind(Kind::Bool)
        .with_shape(ResponseShape::Value { kind: Kind::Bool }),
        "Number" => infer_number_expression_fact(node, parsed),
        "VariableName" => infer_variable_expression_fact(node, parsed),
        "Object" => infer_object_expression_fact(node, parsed, row_table),
        "Array" => infer_array_expression_fact(node, parsed, row_table),
        "BinaryExpression" => infer_binary_expression_fact(node, parsed, row_table),
        _ if is_identifier_like(node) => infer_field_path_expression_fact(node, parsed, row_table),
        _ => ExpressionFact::new(
            node_span(node, parsed.source_id().clone()),
            ExpressionValueClass::Unknown,
        )
        .with_partial(PartialReason::UnsupportedSyntax(node.kind().into())),
    }
}

fn infer_single_child_expression_fact(
    node: Node<'_>,
    parsed: &ParsedSource,
    row_table: Option<&TableDef>,
) -> ExpressionFact {
    let mut cursor = node.walk();
    if let Some(child) = node.children(&mut cursor).find(|child| child.is_named()) {
        return infer_expression_fact(child, parsed, row_table);
    }
    ExpressionFact::new(
        node_span(node, parsed.source_id().clone()),
        ExpressionValueClass::Unknown,
    )
    .with_partial(PartialReason::Unresolved)
}

fn infer_number_expression_fact(node: Node<'_>, parsed: &ParsedSource) -> ExpressionFact {
    let kind = if node_has_child_kind(node, "Float") {
        Kind::Float
    } else {
        Kind::Int
    };
    ExpressionFact::new(
        node_span(node, parsed.source_id().clone()),
        ExpressionValueClass::Literal,
    )
    .with_kind(kind.clone())
    .with_shape(ResponseShape::Value { kind })
}

fn infer_variable_expression_fact(node: Node<'_>, parsed: &ParsedSource) -> ExpressionFact {
    let mut fact = ExpressionFact::new(
        node_span(node, parsed.source_id().clone()),
        ExpressionValueClass::Variable,
    )
    .with_partial(PartialReason::Unresolved);
    fact.dependencies
        .params
        .push(param_name(node_text(node, parsed.text())).to_string());
    fact
}

fn infer_field_path_expression_fact(
    node: Node<'_>,
    parsed: &ParsedSource,
    row_table: Option<&TableDef>,
) -> ExpressionFact {
    let text = node_text(node, parsed.text());
    let mut fact = ExpressionFact::new(
        node_span(node, parsed.source_id().clone()),
        ExpressionValueClass::FieldPath,
    );
    fact.dependencies.field_paths.push(text.to_string());

    let Some(table) = row_table else {
        return fact.with_partial(PartialReason::Unresolved);
    };
    let Some(field) = table.fields.get(text) else {
        return fact.with_partial(PartialReason::Unresolved);
    };
    let Some(kind) = field.kind.clone() else {
        return fact.with_partial(
            field
                .partial
                .first()
                .cloned()
                .unwrap_or(PartialReason::Unresolved),
        );
    };
    fact.with_kind(kind.clone())
        .with_shape(ResponseShape::Value { kind })
}

fn infer_object_expression_fact(
    node: Node<'_>,
    parsed: &ParsedSource,
    row_table: Option<&TableDef>,
) -> ExpressionFact {
    let mut fields = BTreeMap::new();
    let mut partial = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "ObjectContent" && child.kind() != "ObjectProperty" {
            continue;
        }
        collect_object_property_shapes(child, parsed, row_table, &mut fields, &mut partial);
    }

    let mut fact = ExpressionFact::new(
        node_span(node, parsed.source_id().clone()),
        ExpressionValueClass::Object,
    )
    .with_kind(Kind::Object)
    .with_shape(ResponseShape::Object {
        fields,
        open: false,
    });
    fact.partial = partial;
    fact
}

fn collect_object_property_shapes(
    node: Node<'_>,
    parsed: &ParsedSource,
    row_table: Option<&TableDef>,
    fields: &mut BTreeMap<String, FieldShape>,
    partial: &mut Vec<PartialReason>,
) {
    if node.kind() == "ObjectProperty" {
        let Some(key) = object_property_key(node, parsed) else {
            return;
        };
        let Some(value) = object_property_value(node) else {
            partial.push(PartialReason::Unresolved);
            return;
        };
        let value_fact = infer_expression_fact(value, parsed, row_table);
        partial.extend(value_fact.partial.clone());
        fields.insert(
            key.name,
            FieldShape {
                shape: value_fact.shape.unwrap_or(ResponseShape::Unknown {
                    reason: PartialReason::Unresolved,
                }),
                kind: value_fact.kind,
                span: key.span,
                materialized_by_fetch: false,
                partial: value_fact.partial,
            },
        );
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_object_property_shapes(child, parsed, row_table, fields, partial);
    }
}

fn infer_array_expression_fact(
    node: Node<'_>,
    parsed: &ParsedSource,
    row_table: Option<&TableDef>,
) -> ExpressionFact {
    let elements = array_element_nodes(node);
    let element_facts: Vec<_> = elements
        .iter()
        .map(|element| infer_expression_fact(*element, parsed, row_table))
        .collect();
    let max_len = Some(element_facts.len().min(u64::MAX as usize) as u64);

    let element_shape =
        homogeneous_element_shape(&element_facts).unwrap_or(ResponseShape::Unknown {
            reason: PartialReason::UnsupportedSyntax("mixed array".into()),
        });
    let element_kind = homogeneous_element_kind(&element_facts).unwrap_or(Kind::Any);
    let mut fact = ExpressionFact::new(
        node_span(node, parsed.source_id().clone()),
        ExpressionValueClass::Array,
    )
    .with_kind(Kind::Array(Box::new(element_kind), max_len))
    .with_shape(ResponseShape::Array {
        element: Box::new(element_shape),
        max_len,
    });
    fact.partial = element_facts
        .iter()
        .flat_map(|fact| fact.partial.clone())
        .collect();
    if element_facts.len() > 1 && homogeneous_element_kind(&element_facts).is_none() {
        fact.partial
            .push(PartialReason::UnsupportedSyntax("mixed array".into()));
    }
    fact
}

fn infer_binary_expression_fact(
    node: Node<'_>,
    parsed: &ParsedSource,
    row_table: Option<&TableDef>,
) -> ExpressionFact {
    let Some((left, operator, right)) = binary_expression_parts(node) else {
        return ExpressionFact::new(
            node_span(node, parsed.source_id().clone()),
            ExpressionValueClass::Unknown,
        )
        .with_partial(PartialReason::UnsupportedSyntax("BinaryExpression".into()));
    };

    let left_fact = infer_expression_fact(left, parsed, row_table);
    let right_fact = infer_expression_fact(right, parsed, row_table);
    let operator_text = node_text(operator, parsed.text()).trim();

    let mut fact = ExpressionFact::new(
        node_span(node, parsed.source_id().clone()),
        ExpressionValueClass::Unknown,
    );
    fact.dependencies
        .field_paths
        .extend(left_fact.dependencies.field_paths);
    fact.dependencies
        .field_paths
        .extend(right_fact.dependencies.field_paths);
    fact.dependencies
        .params
        .extend(left_fact.dependencies.params);
    fact.dependencies
        .params
        .extend(right_fact.dependencies.params);
    fact.partial.extend(left_fact.partial);
    fact.partial.extend(right_fact.partial);

    if let (Some(left_kind), Some(right_kind)) = (&left_fact.kind, &right_fact.kind) {
        if let Some(kind) = binary_expression_result_kind(operator_text, left_kind, right_kind) {
            fact.kind = Some(kind.clone());
            fact.shape = Some(ResponseShape::Value { kind });
            fact.partial.clear();
            return fact;
        }
    }

    fact.partial
        .push(PartialReason::UnsupportedSyntax("BinaryExpression".into()));
    fact
}

fn binary_expression_parts<'tree>(
    node: Node<'tree>,
) -> Option<(Node<'tree>, Node<'tree>, Node<'tree>)> {
    let mut cursor = node.walk();
    let children: Vec<_> = node
        .children(&mut cursor)
        .filter(|child| child.is_named())
        .collect();
    let operator_index = children
        .iter()
        .position(|child| child.kind() == "Operator")?;
    let left = children[..operator_index]
        .iter()
        .rev()
        .copied()
        .find(|child| child.kind() != "Operator")?;
    let right = children[operator_index + 1..]
        .iter()
        .copied()
        .find(|child| child.kind() != "Operator")?;
    Some((left, children[operator_index], right))
}

fn binary_expression_result_kind(operator: &str, left: &Kind, right: &Kind) -> Option<Kind> {
    match operator {
        "+" if matches!(left, Kind::String) && matches!(right, Kind::String) => Some(Kind::String),
        "+" | "-" | "*" | "/" if is_numeric_kind(left) && is_numeric_kind(right) => {
            if matches!(left, Kind::Float) || matches!(right, Kind::Float) {
                Some(Kind::Float)
            } else {
                Some(Kind::Int)
            }
        }
        _ => None,
    }
}

fn is_numeric_kind(kind: &Kind) -> bool {
    matches!(kind, Kind::Int | Kind::Float | Kind::Number)
}

fn array_element_nodes(array: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = array.walk();
    array
        .children(&mut cursor)
        .filter(|child| child.is_named())
        .collect()
}

fn homogeneous_element_kind(facts: &[ExpressionFact]) -> Option<Kind> {
    let mut kinds = facts.iter().filter_map(|fact| fact.kind.clone());
    let first = kinds.next()?;
    kinds.all(|kind| kind == first).then_some(first)
}

fn homogeneous_element_shape(facts: &[ExpressionFact]) -> Option<ResponseShape> {
    let mut shapes = facts.iter().filter_map(|fact| fact.shape.clone());
    let first = shapes.next()?;
    shapes.all(|shape| shape == first).then_some(first)
}

struct ObjectKey {
    name: String,
    span: SourceSpan,
}

fn object_property_key(property: Node<'_>, parsed: &ParsedSource) -> Option<ObjectKey> {
    let key_node = first_descendant_of_kind(property, "ObjectKey")?;
    let mut cursor = key_node.walk();
    let leaf = key_node
        .children(&mut cursor)
        .find(|child| child.is_named())
        .unwrap_or(key_node);
    Some(ObjectKey {
        name: trim_key_quotes(node_text(leaf, parsed.text())).to_string(),
        span: node_span(leaf, parsed.source_id().clone()),
    })
}

fn object_property_value(property: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = property.walk();
    property
        .children(&mut cursor)
        .filter(|child| child.is_named() && child.kind() != "ObjectKey")
        .last()
}

fn first_descendant_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    if node.kind() == kind {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = first_descendant_of_kind(child, kind) {
            return Some(found);
        }
    }
    None
}

fn node_has_child_kind(node: Node<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    let has_child = node.children(&mut cursor).any(|child| child.kind() == kind);
    has_child
}

fn is_identifier_like(node: Node<'_>) -> bool {
    matches!(node.kind(), "Ident" | "RecordId" | "Thing" | "Identifier")
}

fn param_name(text: &str) -> &str {
    text.trim_start_matches('$')
}

fn trim_key_quotes(text: &str) -> &str {
    text.trim_matches('`').trim_matches('"').trim_matches('\'')
}

fn node_text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    &source[node.start_byte()..node.end_byte()]
}

fn node_span(node: Node<'_>, source: SourceId) -> SourceSpan {
    let start = node.start_byte().min(u32::MAX as usize) as u32;
    let end = node.end_byte().min(u32::MAX as usize) as u32;
    SourceSpan::new(
        source,
        ByteRange::new(start, end).expect("tree-sitter node byte ranges are ordered"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::extract_schema;
    use surrealguard_syntax::parse::{parse_source, ParsedSource};
    use surrealguard_syntax::source::SourceId;
    use surrealguard_syntax::span::ByteRange;
    use tree_sitter::Node;

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

    #[test]
    fn infer_expression_facts_for_basic_literals() {
        let parsed = parse("LET $s = 'hi'; LET $i = 42; LET $f = 1.5; LET $b = true;");

        assert_expression_kind(
            &parsed,
            "String",
            Kind::String,
            ExpressionValueClass::Literal,
        );
        assert_expression_kind(&parsed, "Number", Kind::Int, ExpressionValueClass::Literal);
        assert_expression_kind(&parsed, "Bool", Kind::Bool, ExpressionValueClass::Literal);

        let float_node = find_node_by_text(parsed.tree().root_node(), parsed.text(), "1.5")
            .expect("float literal should parse");
        let float_fact = infer_expression_fact(float_node, &parsed, None);
        assert_eq!(float_fact.kind, Some(Kind::Float));
        assert_eq!(float_fact.value_class, ExpressionValueClass::Literal);
    }

    #[test]
    fn infer_expression_fact_for_schema_backed_row_field_path() {
        let parsed = parse(
            "DEFINE TABLE person SCHEMAFULL; DEFINE FIELD name ON person TYPE string; SELECT name FROM person;",
        );
        let schema = extract_schema(&[parsed]).schema;
        let parsed = parse("SELECT name FROM person;");
        let select = find_first_node(parsed.tree().root_node(), "SelectStatement").unwrap();
        let field = find_node_by_text(select, parsed.text(), "name").unwrap();
        let table = schema.tables.get("person").unwrap();

        let fact = infer_expression_fact(field, &parsed, Some(table));

        assert_eq!(fact.kind, Some(Kind::String));
        assert_eq!(
            fact.shape,
            Some(ResponseShape::Value { kind: Kind::String })
        );
        assert_eq!(fact.value_class, ExpressionValueClass::FieldPath);
        assert_eq!(fact.dependencies.field_paths, vec!["name"]);
    }

    #[test]
    fn infer_expression_facts_for_object_and_array_literals() {
        let parsed = parse(
            "DEFINE TABLE person SCHEMAFULL; DEFINE FIELD name ON person TYPE string; SELECT { name: name, tags: ['a', 'b'], ok: true } AS data FROM person;",
        );
        let schema = extract_schema(&[parsed]).schema;
        let parsed =
            parse("SELECT { name: name, tags: ['a', 'b'], ok: true } AS data FROM person;");
        let object = find_first_node(parsed.tree().root_node(), "Object").unwrap();
        let table = schema.tables.get("person").unwrap();

        let fact = infer_expression_fact(object, &parsed, Some(table));

        assert_eq!(fact.kind, Some(Kind::Object));
        assert_eq!(fact.value_class, ExpressionValueClass::Object);
        let Some(ResponseShape::Object {
            fields,
            open: false,
        }) = fact.shape
        else {
            panic!("object expression should infer closed object shape");
        };
        assert_eq!(fields["name"].kind, Some(Kind::String));
        assert_eq!(fields["ok"].kind, Some(Kind::Bool));
        assert_eq!(
            fields["tags"].kind,
            Some(Kind::Array(Box::new(Kind::String), Some(2)))
        );
    }

    #[test]
    fn infer_expression_fact_for_param_variable_remains_unknown_dependency() {
        let parsed = parse("RETURN $name;");
        let variable = find_first_node(parsed.tree().root_node(), "VariableName").unwrap();

        let fact = infer_expression_fact(variable, &parsed, None);

        assert_eq!(fact.kind, None);
        assert_eq!(fact.value_class, ExpressionValueClass::Variable);
        assert_eq!(fact.dependencies.params, vec!["name"]);
        assert_eq!(fact.partial, vec![PartialReason::Unresolved]);
    }

    fn parse(source: &str) -> ParsedSource {
        parse_source(SourceId::new("query:test"), source).expect("test query should parse")
    }

    fn assert_expression_kind(
        parsed: &ParsedSource,
        node_kind: &str,
        expected_kind: Kind,
        expected_class: ExpressionValueClass,
    ) {
        let node = find_first_node(parsed.tree().root_node(), node_kind)
            .unwrap_or_else(|| panic!("{node_kind} node should parse"));
        let fact = infer_expression_fact(node, parsed, None);
        assert_eq!(fact.kind, Some(expected_kind.clone()));
        assert_eq!(
            fact.shape,
            Some(ResponseShape::Value {
                kind: expected_kind
            })
        );
        assert_eq!(fact.value_class, expected_class);
    }

    fn find_first_node<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
        if node.kind() == kind {
            return Some(node);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if let Some(found) = find_first_node(child, kind) {
                return Some(found);
            }
        }
        None
    }

    fn find_node_by_text<'tree>(
        node: Node<'tree>,
        source: &str,
        expected: &str,
    ) -> Option<Node<'tree>> {
        if node.is_named() && &source[node.start_byte()..node.end_byte()] == expected {
            return Some(node);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if let Some(found) = find_node_by_text(child, source, expected) {
                return Some(found);
            }
        }
        None
    }
}
