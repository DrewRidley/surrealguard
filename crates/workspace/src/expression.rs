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
        "String" => infer_string_expression_fact(node, parsed),
        "Bool" => ExpressionFact::new(
            node_span(node, parsed.source_id().clone()),
            ExpressionValueClass::Literal,
        )
        .with_kind(Kind::Bool)
        .with_shape(ResponseShape::Value { kind: Kind::Bool }),
        "Duration" => scalar_literal_expression_fact(node, parsed, Kind::Duration),
        "Number" => infer_number_expression_fact(node, parsed),
        "None" => infer_none_expression_fact(node, parsed),
        "VariableName" => infer_variable_expression_fact(node, parsed),
        "Object" => infer_object_expression_fact(node, parsed, row_table),
        "Array" => infer_array_expression_fact(node, parsed, row_table),
        "BinaryExpression" => infer_binary_expression_fact(node, parsed, row_table),
        "PrefixExpression" => infer_prefix_expression_fact(node, parsed, row_table),
        "SubQuery" => {
            explicit_partial_expression_fact(node, parsed, ExpressionValueClass::Subquery)
        }
        "Block" => explicit_partial_expression_fact(node, parsed, ExpressionValueClass::Block),
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

fn explicit_partial_expression_fact(
    node: Node<'_>,
    parsed: &ParsedSource,
    value_class: ExpressionValueClass,
) -> ExpressionFact {
    ExpressionFact::new(node_span(node, parsed.source_id().clone()), value_class)
        .with_partial(PartialReason::UnsupportedSyntax(node.kind().into()))
}

fn infer_string_expression_fact(node: Node<'_>, parsed: &ParsedSource) -> ExpressionFact {
    let text = node_text(node, parsed.text()).trim_start();
    let kind = match text
        .as_bytes()
        .first()
        .map(|byte| byte.to_ascii_lowercase())
    {
        Some(b'd') if prefixed_string_literal(text) => Kind::Datetime,
        Some(b'u') if prefixed_string_literal(text) => Kind::Uuid,
        Some(b'r') if prefixed_string_literal(text) => Kind::Regex,
        _ => Kind::String,
    };
    scalar_literal_expression_fact(node, parsed, kind)
}

fn infer_number_expression_fact(node: Node<'_>, parsed: &ParsedSource) -> ExpressionFact {
    let kind = if node_has_child_kind(node, "Decimal") {
        Kind::Decimal
    } else if node_has_child_kind(node, "Float") {
        Kind::Float
    } else {
        Kind::Int
    };
    scalar_literal_expression_fact(node, parsed, kind)
}

fn scalar_literal_expression_fact(
    node: Node<'_>,
    parsed: &ParsedSource,
    kind: Kind,
) -> ExpressionFact {
    ExpressionFact::new(
        node_span(node, parsed.source_id().clone()),
        ExpressionValueClass::Literal,
    )
    .with_kind(kind.clone())
    .with_shape(ResponseShape::Value { kind })
}

fn infer_none_expression_fact(node: Node<'_>, parsed: &ParsedSource) -> ExpressionFact {
    let kind = if node_text(node, parsed.text()).eq_ignore_ascii_case("null") {
        Kind::Null
    } else {
        Kind::None
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

fn infer_prefix_expression_fact(
    node: Node<'_>,
    parsed: &ParsedSource,
    row_table: Option<&TableDef>,
) -> ExpressionFact {
    let mut cursor = node.walk();
    let children: Vec<_> = node
        .children(&mut cursor)
        .filter(|child| child.is_named())
        .collect();
    let operator = children
        .iter()
        .copied()
        .find(|child| child.kind() == "Operator");
    let operand = children
        .iter()
        .copied()
        .find(|child| child.kind() != "Operator");
    let Some((operator, operand)) = operator.zip(operand) else {
        return ExpressionFact::new(
            node_span(node, parsed.source_id().clone()),
            ExpressionValueClass::Unknown,
        )
        .with_partial(PartialReason::UnsupportedSyntax("PrefixExpression".into()));
    };

    let operand_fact = infer_expression_fact(operand, parsed, row_table);
    let operator_text = node_text(operator, parsed.text())
        .trim()
        .to_ascii_uppercase();
    let mut fact = ExpressionFact::new(
        node_span(node, parsed.source_id().clone()),
        ExpressionValueClass::Unknown,
    );
    fact.dependencies = operand_fact.dependencies;
    fact.partial = operand_fact.partial;

    if operator_text == "!" && matches!(operand_fact.kind, Some(Kind::Bool)) {
        fact.kind = Some(Kind::Bool);
        fact.shape = Some(ResponseShape::Value { kind: Kind::Bool });
        fact.partial.clear();
        return fact;
    }
    if matches!(operator_text.as_str(), "+" | "-") {
        if let Some(kind) = operand_fact.kind.filter(is_numeric_kind) {
            fact.kind = Some(kind.clone());
            fact.shape = Some(ResponseShape::Value { kind });
            fact.partial.clear();
            return fact;
        }
    }

    fact.partial
        .push(PartialReason::UnsupportedSyntax("PrefixExpression".into()));
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
    match operator.to_ascii_uppercase().as_str() {
        "+" if matches!(left, Kind::String) && matches!(right, Kind::String) => Some(Kind::String),
        "+" | "-" | "*" | "/" if is_numeric_kind(left) && is_numeric_kind(right) => {
            if matches!(left, Kind::Decimal) || matches!(right, Kind::Decimal) {
                Some(Kind::Decimal)
            } else if matches!(left, Kind::Float) || matches!(right, Kind::Float) {
                Some(Kind::Float)
            } else {
                Some(Kind::Int)
            }
        }
        "=" | "==" | "!=" | "<" | "<=" | ">" | ">=" if comparable_binary_kinds(left, right) => {
            Some(Kind::Bool)
        }
        "AND" | "OR" if matches!(left, Kind::Bool) && matches!(right, Kind::Bool) => {
            Some(Kind::Bool)
        }
        "??" if matches!(left, Kind::None | Kind::Null) => Some(right.clone()),
        "??" if matches!(right, Kind::None | Kind::Null) => Some(left.clone()),
        "??" if left == right => Some(left.clone()),
        _ => None,
    }
}

fn comparable_binary_kinds(left: &Kind, right: &Kind) -> bool {
    left == right || (is_numeric_kind(left) && is_numeric_kind(right))
}

fn is_numeric_kind(kind: &Kind) -> bool {
    matches!(kind, Kind::Int | Kind::Float | Kind::Decimal | Kind::Number)
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

fn prefixed_string_literal(text: &str) -> bool {
    text.len() > 2 && matches!(text.as_bytes().get(1), Some(b'\'') | Some(b'\"'))
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
    fn infer_expression_facts_for_decimal_and_duration_literals() {
        let parsed = parse("RETURN [1dec, 1h];");
        let decimal_node = find_node_by_text(parsed.tree().root_node(), parsed.text(), "1dec")
            .expect("decimal literal should parse");
        let duration_node = find_node_by_text(parsed.tree().root_node(), parsed.text(), "1h")
            .expect("duration literal should parse");

        let decimal_fact = infer_expression_fact(decimal_node, &parsed, None);
        let duration_fact = infer_expression_fact(duration_node, &parsed, None);

        assert_eq!(decimal_fact.kind, Some(Kind::Decimal));
        assert_eq!(
            decimal_fact.shape,
            Some(ResponseShape::Value {
                kind: Kind::Decimal
            })
        );
        assert_eq!(decimal_fact.value_class, ExpressionValueClass::Literal);
        assert_eq!(duration_fact.kind, Some(Kind::Duration));
        assert_eq!(
            duration_fact.shape,
            Some(ResponseShape::Value {
                kind: Kind::Duration
            })
        );
        assert_eq!(duration_fact.value_class, ExpressionValueClass::Literal);
    }

    #[test]
    fn infer_expression_facts_for_null_and_none_literals() {
        let parsed = parse("RETURN [null, NONE];");

        let null_node = find_node_by_text(parsed.tree().root_node(), parsed.text(), "null")
            .expect("null literal should parse");
        let none_node = find_node_by_text(parsed.tree().root_node(), parsed.text(), "NONE")
            .expect("NONE literal should parse");

        let null_fact = infer_expression_fact(null_node, &parsed, None);
        let none_fact = infer_expression_fact(none_node, &parsed, None);

        assert_eq!(null_fact.kind, Some(Kind::Null));
        assert_eq!(
            null_fact.shape,
            Some(ResponseShape::Value { kind: Kind::Null })
        );
        assert_eq!(null_fact.value_class, ExpressionValueClass::Literal);
        assert_eq!(none_fact.kind, Some(Kind::None));
        assert_eq!(
            none_fact.shape,
            Some(ResponseShape::Value { kind: Kind::None })
        );
        assert_eq!(none_fact.value_class, ExpressionValueClass::Literal);
    }

    #[test]
    fn infer_expression_facts_for_prefixed_string_literals() {
        let parsed = parse(
            "RETURN [d'2024-01-01T00:00:00Z', u'01890f2e-7cc0-7d78-9c3b-49f9f6e0f012', r'abc'];",
        );
        let datetime_node = find_node_by_text(
            parsed.tree().root_node(),
            parsed.text(),
            "d'2024-01-01T00:00:00Z'",
        )
        .expect("datetime literal should parse");
        let uuid_node = find_node_by_text(
            parsed.tree().root_node(),
            parsed.text(),
            "u'01890f2e-7cc0-7d78-9c3b-49f9f6e0f012'",
        )
        .expect("uuid literal should parse");
        let regex_node = find_node_by_text(parsed.tree().root_node(), parsed.text(), "r'abc'")
            .expect("regex literal should parse");

        assert_eq!(
            infer_expression_fact(datetime_node, &parsed, None).kind,
            Some(Kind::Datetime)
        );
        assert_eq!(
            infer_expression_fact(uuid_node, &parsed, None).kind,
            Some(Kind::Uuid)
        );
        assert_eq!(
            infer_expression_fact(regex_node, &parsed, None).kind,
            Some(Kind::Regex)
        );
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
    fn infer_expression_fact_for_comparison_operators_returns_bool() {
        let parsed = parse(
            "DEFINE TABLE person SCHEMAFULL; DEFINE FIELD age ON person TYPE int; SELECT age >= 18 AS adult FROM person;",
        );
        let schema = extract_schema(&[parsed]).schema;
        let parsed = parse("SELECT age >= 18 AS adult FROM person;");
        let comparison = find_node_by_text(parsed.tree().root_node(), parsed.text(), "age >= 18")
            .expect("comparison expression should parse");
        let table = schema.tables.get("person").unwrap();

        let fact = infer_expression_fact(comparison, &parsed, Some(table));

        assert_eq!(fact.kind, Some(Kind::Bool));
        assert_eq!(fact.shape, Some(ResponseShape::Value { kind: Kind::Bool }));
        assert_eq!(fact.dependencies.field_paths, vec!["age"]);
        assert!(
            fact.partial.is_empty(),
            "comparison should be fully inferred"
        );
    }

    #[test]
    fn infer_expression_fact_for_boolean_operators_returns_bool() {
        let parsed = parse("RETURN true AND false;");
        let boolean_expr =
            find_node_by_text(parsed.tree().root_node(), parsed.text(), "true AND false")
                .expect("boolean operator expression should parse");

        let fact = infer_expression_fact(boolean_expr, &parsed, None);

        assert_eq!(fact.kind, Some(Kind::Bool));
        assert_eq!(fact.shape, Some(ResponseShape::Value { kind: Kind::Bool }));
        assert!(
            fact.partial.is_empty(),
            "boolean operator should be fully inferred"
        );
    }

    #[test]
    fn infer_expression_fact_for_prefix_not_returns_bool() {
        let parsed = parse("RETURN !true;");
        let prefix = find_node_by_text(parsed.tree().root_node(), parsed.text(), "!true")
            .expect("prefix expression should parse");

        let fact = infer_expression_fact(prefix, &parsed, None);

        assert_eq!(fact.kind, Some(Kind::Bool));
        assert_eq!(fact.shape, Some(ResponseShape::Value { kind: Kind::Bool }));
        assert!(
            fact.partial.is_empty(),
            "known prefix expression should be fully inferred"
        );
    }

    #[test]
    fn infer_expression_fact_for_coalesce_returns_known_operand_kind() {
        let parsed = parse("RETURN NONE ?? 'fallback';");
        let coalesce = find_node_by_text(
            parsed.tree().root_node(),
            parsed.text(),
            "NONE ?? 'fallback'",
        )
        .expect("coalesce expression should parse");

        let fact = infer_expression_fact(coalesce, &parsed, None);

        assert_eq!(fact.kind, Some(Kind::String));
        assert_eq!(
            fact.shape,
            Some(ResponseShape::Value { kind: Kind::String })
        );
        assert!(
            fact.partial.is_empty(),
            "known coalesce expression should be fully inferred"
        );
    }

    #[test]
    fn infer_expression_facts_for_subqueries_and_blocks_are_explicit_partials() {
        let parsed = parse("RETURN [(SELECT * FROM person), { LET $x = 1; RETURN $x; }];");
        let subquery = find_first_node(parsed.tree().root_node(), "SubQuery")
            .expect("subquery expression should parse");
        let block = find_first_node(parsed.tree().root_node(), "Block")
            .expect("block expression should parse");

        let subquery_fact = infer_expression_fact(subquery, &parsed, None);
        let block_fact = infer_expression_fact(block, &parsed, None);

        assert_eq!(subquery_fact.value_class, ExpressionValueClass::Subquery);
        assert_eq!(
            subquery_fact.partial,
            vec![PartialReason::UnsupportedSyntax("SubQuery".into())]
        );
        assert_eq!(block_fact.value_class, ExpressionValueClass::Block);
        assert_eq!(
            block_fact.partial,
            vec![PartialReason::UnsupportedSyntax("Block".into())]
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
