//! The pre-AST SELECT intermediate representation. Serves only the frozen
//! validators in [`crate::semantic`]; dies with them.

use surrealguard_syntax::parse::ParsedSource;
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};
use tree_sitter::Node;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectIr {
    pub source: Option<SelectSource>,
    pub projections: Vec<SelectProjection>,
    pub only: bool,
    pub omit: Vec<FieldPath>,
    pub fetch: Vec<FieldPath>,
    pub split: Vec<FieldPath>,
    pub return_clause: Option<String>,
    pub modifiers: Vec<SelectModifier>,
    pub graph_lookups: Vec<GraphLookup>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectSource {
    pub table: Option<String>,
    pub span: SourceSpan,
    pub dynamic: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectProjection {
    Wildcard {
        span: SourceSpan,
    },
    Field {
        path: FieldPath,
        span: SourceSpan,
        alias: Option<String>,
        value: bool,
    },
    Dynamic {
        span: SourceSpan,
        alias: Option<String>,
        expression_kind: Option<String>,
        expression_text: String,
        value: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldPath {
    pub text: String,
    pub segments: Vec<String>,
    pub span: SourceSpan,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectModifier {
    Where(SourceSpan),
    Order(SourceSpan),
    Limit {
        span: SourceSpan,
        max_len: Option<u64>,
    },
    Start(SourceSpan),
    Timeout(SourceSpan),
    Parallel(SourceSpan),
    Group(SourceSpan),
    Split(SourceSpan),
    Explain(SourceSpan),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphLookup {
    pub direction: GraphDirection,
    pub table: Option<String>,
    pub span: SourceSpan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphDirection {
    Out,
    In,
    Both,
}

pub fn extract_select_ir(parsed: &ParsedSource) -> Vec<SelectIr> {
    let mut irs = Vec::new();
    collect_select_irs(
        parsed.tree().root_node(),
        parsed.source_id(),
        parsed.text(),
        &mut irs,
    );
    irs
}

fn collect_select_irs(node: Node<'_>, source: &SourceId, text: &str, irs: &mut Vec<SelectIr>) {
    if node.kind() == "SelectStatement" {
        irs.push(select_ir_from_statement(node, source, text));
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_select_irs(child, source, text, irs);
    }
}

pub(crate) fn select_ir_from_statement(node: Node<'_>, source: &SourceId, text: &str) -> SelectIr {
    let mut select_source = None;
    let mut projections = Vec::new();
    let mut only = false;
    let mut omit = Vec::new();
    let mut fetch = Vec::new();
    let mut split = Vec::new();
    let mut return_clause = None;
    let mut modifiers = Vec::new();
    let mut graph_lookups = Vec::new();
    let mut saw_from = false;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "Fields" => projections = projections_from_fields(child, source, text),
            "OmitClause" => omit = field_paths_from_clause(child, source, text),
            "FetchClause" => fetch = field_paths_from_clause(child, source, text),
            "ReturnClause" => return_clause = Some(node_text(child, text).trim().to_string()),
            "WhereClause" => {
                modifiers.push(SelectModifier::Where(node_span(child, source.clone())))
            }
            "OrderClause" => {
                modifiers.push(SelectModifier::Order(node_span(child, source.clone())))
            }
            "LimitClause" => modifiers.push(SelectModifier::Limit {
                span: node_span(child, source.clone()),
                max_len: literal_limit_from_clause(child, text),
            }),
            "StartClause" => {
                modifiers.push(SelectModifier::Start(node_span(child, source.clone())))
            }
            "TimeoutClause" => {
                modifiers.push(SelectModifier::Timeout(node_span(child, source.clone())))
            }
            "ParallelClause" => {
                modifiers.push(SelectModifier::Parallel(node_span(child, source.clone())))
            }
            "GroupClause" => {
                modifiers.push(SelectModifier::Group(node_span(child, source.clone())))
            }
            "SplitClause" => {
                split = field_paths_from_clause(child, source, text);
                modifiers.push(SelectModifier::Split(node_span(child, source.clone())));
            }
            "ExplainClause" => {
                modifiers.push(SelectModifier::Explain(node_span(child, source.clone())))
            }
            "LimitStartComboClause" => {
                collect_modifier_clauses(child, source, text, &mut modifiers)
            }
            "Keyword" => {
                let keyword = node_text(child, text).to_ascii_lowercase();
                if keyword == "from" {
                    saw_from = true;
                } else if saw_from && keyword == "only" {
                    only = true;
                }
            }
            _ if saw_from && select_source.is_none() && is_select_source_node(child) => {
                let (source_value, lookups) = source_from_node(child, source, text);
                select_source = Some(source_value);
                graph_lookups = lookups;
            }
            _ => {}
        }
    }

    SelectIr {
        source: select_source,
        projections,
        only,
        omit,
        fetch,
        split,
        return_clause,
        modifiers,
        graph_lookups,
    }
}

fn collect_modifier_clauses(
    node: Node<'_>,
    source: &SourceId,
    text: &str,
    modifiers: &mut Vec<SelectModifier>,
) {
    match node.kind() {
        "LimitClause" => modifiers.push(SelectModifier::Limit {
            span: node_span(node, source.clone()),
            max_len: literal_limit_from_clause(node, text),
        }),
        "StartClause" => modifiers.push(SelectModifier::Start(node_span(node, source.clone()))),
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                collect_modifier_clauses(child, source, text, modifiers);
            }
        }
    }
}

fn literal_limit_from_clause(clause: Node<'_>, text: &str) -> Option<u64> {
    let mut cursor = clause.walk();
    for child in clause.children(&mut cursor) {
        if child.kind() == "Number" || child.kind() == "Int" {
            if let Some(limit) = literal_limit_from_clause(child, text) {
                return Some(limit);
            }
            let child_text = node_text(child, text).trim();
            if let Ok(limit) = child_text.parse::<u64>() {
                return Some(limit);
            }
        }
    }
    let clause_text = node_text(clause, text).trim();
    clause_text.parse::<u64>().ok()
}

fn projections_from_fields(
    fields_node: Node<'_>,
    source: &SourceId,
    text: &str,
) -> Vec<SelectProjection> {
    let mut projections = Vec::new();
    let mut value = false;
    let mut cursor = fields_node.walk();

    for child in fields_node.children(&mut cursor) {
        if child.kind() == "Keyword" && node_text(child, text).eq_ignore_ascii_case("value") {
            value = true;
            continue;
        }
        if child.kind() == "Any" {
            projections.push(SelectProjection::Wildcard {
                span: node_span(child, source.clone()),
            });
            continue;
        }
        if child.kind() == "Predicate" {
            projections.push(projection_from_predicate(child, source, text, value));
        }
    }

    projections
}

fn projection_from_predicate(
    predicate: Node<'_>,
    source: &SourceId,
    text: &str,
    value: bool,
) -> SelectProjection {
    let mut field_node = None;
    let mut expression_node = None;
    let mut alias = None;
    let mut saw_as = false;
    let mut cursor = predicate.walk();

    for child in predicate
        .children(&mut cursor)
        .filter(|child| child.is_named())
    {
        if child.kind() == "Keyword" && node_text(child, text).eq_ignore_ascii_case("as") {
            saw_as = true;
            continue;
        }
        if saw_as && is_identifier_like(child) {
            alias = Some(node_text(child, text).trim().to_string());
            continue;
        }
        if expression_node.is_none() && !matches!(child.kind(), "Keyword") {
            expression_node = Some(child);
        }
        if field_node.is_none() && is_field_path_node(child) {
            field_node = Some(child);
        }
    }

    let Some(field_node) = field_node else {
        let expression = expression_node.unwrap_or(predicate);
        return SelectProjection::Dynamic {
            span: node_span(expression, source.clone()),
            alias,
            expression_kind: Some(expression.kind().to_string()),
            expression_text: node_text(expression, text).trim().to_string(),
            value,
        };
    };

    let path = field_path_from_node(field_node, source, text);
    SelectProjection::Field {
        span: path.span.clone(),
        path,
        alias,
        value,
    }
}

fn field_paths_from_clause(clause: Node<'_>, source: &SourceId, text: &str) -> Vec<FieldPath> {
    let mut paths = Vec::new();
    let mut cursor = clause.walk();
    for child in clause.children(&mut cursor) {
        collect_field_paths(child, source, text, &mut paths);
    }
    paths
}

fn collect_field_paths(node: Node<'_>, source: &SourceId, text: &str, paths: &mut Vec<FieldPath>) {
    if is_field_path_node(node) {
        paths.push(field_path_from_node(node, source, text));
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_field_paths(child, source, text, paths);
    }
}

fn source_from_node(
    node: Node<'_>,
    source: &SourceId,
    text: &str,
) -> (SelectSource, Vec<GraphLookup>) {
    if node.kind() == "Path" {
        let mut source_node = None;
        let mut lookups = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if source_node.is_none() && is_identifier_like(child) {
                source_node = Some(child);
                continue;
            }
            if child.kind() == "Lookup" {
                lookups.push(graph_lookup_from_node(child, source, text));
            }
        }
        let source_node = source_node.unwrap_or(node);
        return (
            SelectSource {
                table: Some(table_name_from_node_text(node_text(source_node, text)).to_string()),
                span: node_span(source_node, source.clone()),
                dynamic: false,
            },
            lookups,
        );
    }

    (
        SelectSource {
            table: Some(table_name_from_node_text(node_text(node, text)).to_string()),
            span: node_span(node, source.clone()),
            dynamic: false,
        },
        Vec::new(),
    )
}

fn graph_lookup_from_node(node: Node<'_>, source: &SourceId, text: &str) -> GraphLookup {
    let mut direction = GraphDirection::Out;
    let mut table = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "LookupRight" => direction = GraphDirection::Out,
            "LookupLeft" => direction = GraphDirection::In,
            "LookupBoth" => direction = GraphDirection::Both,
            _ if table.is_none() => table = first_identifier_like_text(child, text),
            _ => {}
        }
    }

    GraphLookup {
        direction,
        table,
        span: node_span(node, source.clone()),
    }
}

fn first_identifier_like_text(node: Node<'_>, text: &str) -> Option<String> {
    if is_identifier_like(node) {
        return Some(node_text(node, text).trim().to_string());
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = first_identifier_like_text(child, text) {
            return Some(found);
        }
    }
    None
}

fn field_path_from_node(node: Node<'_>, source: &SourceId, text: &str) -> FieldPath {
    let path_text = node_text(node, text).trim().to_string();
    FieldPath {
        segments: path_text.split('.').map(str::to_string).collect(),
        text: path_text,
        span: node_span(node, source.clone()),
    }
}

fn is_select_source_node(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "Ident" | "RecordId" | "Thing" | "Identifier" | "Path"
    )
}

fn is_identifier_like(node: Node<'_>) -> bool {
    matches!(node.kind(), "Ident" | "RecordId" | "Thing" | "Identifier")
}

fn is_field_path_node(node: Node<'_>) -> bool {
    matches!(node.kind(), "Ident" | "Path" | "Idiom")
}

fn table_name_from_node_text(text: &str) -> &str {
    text.split_once(':')
        .map(|(table, _)| table)
        .unwrap_or(text)
        .trim()
}

fn node_text<'source>(node: Node<'_>, source: &'source str) -> &'source str {
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
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    fn select_irs(query: &str) -> Vec<SelectIr> {
        let parsed = parse_source(SourceId::new("test"), query).expect("query parses");
        assert!(parsed.syntax_diagnostics().is_empty());
        extract_select_ir(&parsed)
    }

    fn first_ir(query: &str) -> SelectIr {
        let mut irs = select_irs(query);
        assert_eq!(irs.len(), 1);
        irs.remove(0)
    }

    #[test]
    fn extracts_wildcard_select_source() {
        let ir = first_ir("SELECT * FROM person;");

        assert_eq!(
            ir.source
                .as_ref()
                .and_then(|source| source.table.as_deref()),
            Some("person")
        );
        assert!(!ir.only);
        assert_eq!(ir.projections.len(), 1);
        assert!(matches!(
            ir.projections[0],
            SelectProjection::Wildcard { .. }
        ));
    }

    #[test]
    fn extracts_named_and_nested_projection_paths() {
        let ir = first_ir("SELECT name, profile.email FROM person;");

        let projections: Vec<_> = ir
            .projections
            .iter()
            .map(|projection| match projection {
                SelectProjection::Field {
                    path, alias, value, ..
                } => (
                    path.text.as_str(),
                    path.segments.clone(),
                    alias.as_deref(),
                    *value,
                ),
                other => panic!("unexpected projection: {other:?}"),
            })
            .collect();
        assert_eq!(
            projections,
            vec![
                ("name", vec!["name".into()], None, false),
                (
                    "profile.email",
                    vec!["profile".into(), "email".into()],
                    None,
                    false
                ),
            ]
        );
    }

    #[test]
    fn extracts_alias_and_value_projection_flags() {
        let alias = first_ir("SELECT name AS display_name FROM person;");
        assert_eq!(
            alias.projections,
            vec![SelectProjection::Field {
                path: FieldPath {
                    text: "name".into(),
                    segments: vec!["name".into()],
                    span: alias.projections[0].span().clone(),
                },
                span: alias.projections[0].span().clone(),
                alias: Some("display_name".into()),
                value: false,
            }]
        );

        let value = first_ir("SELECT VALUE name FROM person;");
        assert!(matches!(
            &value.projections[0],
            SelectProjection::Field { path, value: true, alias: None, .. } if path.text == "name"
        ));
    }

    #[test]
    fn extracts_omit_fetch_and_only() {
        let omit = first_ir("SELECT * OMIT password FROM person;");
        assert_eq!(
            omit.omit
                .iter()
                .map(|path| path.text.as_str())
                .collect::<Vec<_>>(),
            vec!["password"]
        );

        let fetch = first_ir("SELECT * FROM person FETCH profile;");
        assert_eq!(
            fetch
                .fetch
                .iter()
                .map(|path| path.text.as_str())
                .collect::<Vec<_>>(),
            vec!["profile"]
        );

        let split = first_ir("SELECT tags FROM person SPLIT tags;");
        assert_eq!(
            split
                .split
                .iter()
                .map(|path| path.text.as_str())
                .collect::<Vec<_>>(),
            vec!["tags"]
        );

        let only = first_ir("SELECT * FROM ONLY person:one;");
        assert!(only.only);
        assert_eq!(
            only.source
                .as_ref()
                .and_then(|source| source.table.as_deref()),
            Some("person")
        );
    }

    #[test]
    fn extracts_graph_lookup_nodes_as_partial_ir() {
        let ir = first_ir("SELECT * FROM person->likes->post;");

        assert_eq!(
            ir.source
                .as_ref()
                .and_then(|source| source.table.as_deref()),
            Some("person")
        );
        assert_eq!(ir.graph_lookups.len(), 2);
        assert_eq!(ir.graph_lookups[0].direction, GraphDirection::Out);
        assert_eq!(ir.graph_lookups[0].table.as_deref(), Some("likes"));
        assert_eq!(ir.graph_lookups[1].table.as_deref(), Some("post"));
    }

    #[test]
    fn extracts_graph_projection_with_brace_selector() {
        let ir = first_ir("SELECT ->friend->user.{name, age} FROM person;");

        assert_eq!(ir.projections.len(), 1);
        let SelectProjection::Field { path, alias, .. } = &ir.projections[0] else {
            panic!("expected field projection, got {:?}", ir.projections[0]);
        };
        assert_eq!(path.text, "->friend->user.{name, age}");
        assert_eq!(
            path.segments,
            vec!["->friend->user".to_string(), "{name, age}".to_string(),]
        );
        assert_eq!(alias, &None);
    }

    #[test]
    fn extracts_aliased_graph_projection_with_brace_selector() {
        let ir = first_ir("SELECT ->friend->user.{name, age} AS friends FROM person;");

        assert_eq!(ir.projections.len(), 1);
        let SelectProjection::Field { path, alias, .. } = &ir.projections[0] else {
            panic!("expected field projection, got {:?}", ir.projections[0]);
        };
        assert_eq!(path.text, "->friend->user.{name, age}");
        assert_eq!(
            path.segments,
            vec!["->friend->user".to_string(), "{name, age}".to_string(),]
        );
        assert_eq!(alias.as_deref(), Some("friends"));
    }

    #[test]
    fn extracts_parenthesized_graph_lookup_selection_table() {
        let ir = first_ir("SELECT * FROM person->(likes WHERE created_at > $since)->post;");

        assert_eq!(ir.graph_lookups.len(), 2);
        assert_eq!(ir.graph_lookups[0].direction, GraphDirection::Out);
        assert_eq!(ir.graph_lookups[0].table.as_deref(), Some("likes"));
        assert_eq!(ir.graph_lookups[1].table.as_deref(), Some("post"));
    }

    trait ProjectionSpan {
        fn span(&self) -> &SourceSpan;
    }

    impl ProjectionSpan for SelectProjection {
        fn span(&self) -> &SourceSpan {
            match self {
                SelectProjection::Wildcard { span }
                | SelectProjection::Field { span, .. }
                | SelectProjection::Dynamic { span, .. } => span,
            }
        }
    }
}
