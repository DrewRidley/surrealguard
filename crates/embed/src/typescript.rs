//! `surql` tagged-template extraction over the TypeScript grammar.

use tree_sitter::{Node, Parser};

use crate::{EmbeddedQuery, Segment, Substitution};

/// Finds every `` surql`...` `` template in a TypeScript source. `tsx`
/// selects the TSX grammar (needed for files with JSX).
pub fn extract_typescript(text: &str, tsx: bool) -> Vec<EmbeddedQuery> {
    let language = if tsx {
        tree_sitter_typescript::LANGUAGE_TSX
    } else {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT
    };
    let mut parser = Parser::new();
    if parser.set_language(&language.into()).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(text, None) else {
        return Vec::new();
    };

    let mut queries = Vec::new();
    collect(tree.root_node(), text, &mut queries);
    queries
}

fn collect(node: Node<'_>, text: &str, queries: &mut Vec<EmbeddedQuery>) {
    if node.kind() == "call_expression" {
        if let (Some(function), Some(arguments)) = (
            node.child_by_field_name("function"),
            node.child_by_field_name("arguments"),
        ) {
            if is_surql_tag(function, text) {
                if arguments.kind() == "template_string" {
                    if let Some(query) = template_to_query(arguments, text) {
                        queries.push(query);
                    }
                } else if let Some(query) = call_string_to_query(arguments, text) {
                    queries.push(query);
                }
            } else if is_query_method(function, text) {
                // The runtime API: `db.query("...")` / `db.live("...")`. The
                // first string (or template) argument is the query; a trailing
                // bindings object is ignored. This is what makes the generated
                // registry cover real client code, not just `surql` templates.
                if let Some(query) = method_call_to_query(arguments, text) {
                    queries.push(query);
                }
            }
        }
    }
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();
    for child in children {
        collect(child, text, queries);
    }
}

/// The tag is `surql` itself or a `.surql` member (`db.surql`, ...).
fn is_surql_tag(node: Node<'_>, text: &str) -> bool {
    match node.kind() {
        "identifier" => &text[node.byte_range()] == "surql",
        "member_expression" => node
            .child_by_field_name("property")
            .is_some_and(|property| &text[property.byte_range()] == "surql"),
        _ => false,
    }
}

/// The call form `surql("...")`: one plain string-literal argument. This
/// is the *typed* form — string literals resolve through the generated
/// registry, which tagged templates cannot (TypeScript never infers
/// literal types for template string arrays).
fn call_string_to_query(arguments: Node<'_>, text: &str) -> Option<EmbeddedQuery> {
    if arguments.kind() != "arguments" {
        return None;
    }
    let mut walker = arguments.walk();
    let literals: Vec<Node<'_>> = arguments
        .children(&mut walker)
        .filter(|child| child.kind() == "string")
        .collect();
    let [string] = literals.as_slice() else {
        return None;
    };
    string_node_to_query(*string, text)
}

/// The function of a call is a `.query`/`.live` member access — the
/// SurrealDB runtime API (`db.query(...)`, `db.live(...)`). We match any
/// receiver rather than a fixed `db` name; the string-literal-first-arg
/// requirement keeps unrelated `.query(...)` calls (which pass options
/// objects, not string literals) from matching.
fn is_query_method(node: Node<'_>, text: &str) -> bool {
    node.kind() == "member_expression"
        && node.child_by_field_name("property").is_some_and(|property| {
            matches!(&text[property.byte_range()], "query" | "live")
        })
}

/// `db.query("...")` / `db.live("...")`: the first string (or template)
/// argument is the query; a trailing bindings object is ignored.
fn method_call_to_query(arguments: Node<'_>, text: &str) -> Option<EmbeddedQuery> {
    if arguments.kind() != "arguments" {
        return None;
    }
    let mut walker = arguments.walk();
    let first = arguments
        .children(&mut walker)
        .find(|child| matches!(child.kind(), "string" | "template_string"))?;
    match first.kind() {
        "template_string" => template_to_query(first, text),
        _ => string_node_to_query(first, text),
    }
}

/// Extracts a single `string` literal node's contents as a query.
/// Content between the quotes; escape sequences pass through verbatim
/// (SurrealQL strings share the common escapes).
fn string_node_to_query(string: Node<'_>, text: &str) -> Option<EmbeddedQuery> {
    let content_start = string.start_byte() + 1;
    let content_end = string.end_byte().saturating_sub(1);
    if content_end < content_start {
        return None;
    }
    let mut query = String::new();
    let mut segments = Vec::new();
    push_fragment(text, content_start..content_end, &mut query, &mut segments);
    Some(EmbeddedQuery {
        text: query,
        host_range: content_start..content_end,
        segments,
        substitutions: Vec::new(),
    })
}

/// Rebuilds the template's contents as analyzable SurrealQL: string
/// fragments copy verbatim (with a segment-map entry each), and every
/// `${...}` substitution becomes a `$__hostN` parameter.
fn template_to_query(template: Node<'_>, text: &str) -> Option<EmbeddedQuery> {
    let content_start = template.start_byte() + 1;
    let content_end = template.end_byte().saturating_sub(1);
    if content_end < content_start {
        return None;
    }

    let mut query = String::new();
    let mut segments = Vec::new();
    let mut substitutions = Vec::new();
    let mut host_cursor = content_start;

    let mut walker = template.walk();
    let children: Vec<_> = template.children(&mut walker).collect();
    for child in children {
        if child.kind() != "template_substitution" {
            continue;
        }
        push_fragment(
            text,
            host_cursor..child.start_byte(),
            &mut query,
            &mut segments,
        );
        let param = format!("__host{}", substitutions.len());
        query.push('$');
        query.push_str(&param);
        substitutions.push(Substitution {
            param,
            host_range: child.byte_range(),
        });
        host_cursor = child.end_byte();
    }
    push_fragment(text, host_cursor..content_end, &mut query, &mut segments);

    Some(EmbeddedQuery {
        text: query,
        host_range: content_start..content_end,
        segments,
        substitutions,
    })
}

fn push_fragment(
    text: &str,
    host_range: std::ops::Range<usize>,
    query: &mut String,
    segments: &mut Vec<Segment>,
) {
    if host_range.is_empty() {
        return;
    }
    segments.push(Segment {
        embed_start: query.len(),
        host_start: host_range.start,
        len: host_range.len(),
    });
    query.push_str(&text[host_range]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_surql_tagged_templates() {
        let source = r#"
const name = "Ada";
const q = surql`SELECT * FROM person`;
const other = css`b { color: red }`;
"#;
        let queries = extract_typescript(source, false);

        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].text, "SELECT * FROM person");
        assert_eq!(
            &source[queries[0].host_range.clone()],
            "SELECT * FROM person"
        );
        assert!(queries[0].substitutions.is_empty());
    }

    #[test]
    fn substitutions_become_parameters_with_host_ranges() {
        let source = "const q = surql`SELECT * FROM person WHERE age > ${min} AND name = ${name}`;";
        let queries = extract_typescript(source, false);

        assert_eq!(queries.len(), 1);
        assert_eq!(
            queries[0].text,
            "SELECT * FROM person WHERE age > $__host0 AND name = $__host1"
        );
        assert_eq!(queries[0].substitutions.len(), 2);
        assert_eq!(
            &source[queries[0].substitutions[0].host_range.clone()],
            "${min}"
        );
        assert_eq!(
            &source[queries[0].substitutions[1].host_range.clone()],
            "${name}"
        );
    }

    #[test]
    fn offsets_map_through_substitutions() {
        let source = "const q = surql`SELECT * FROM person WHERE age > ${min} AND name = 'x'`;";
        let queries = extract_typescript(source, false);
        let query = &queries[0];

        // `person` before the substitution maps verbatim.
        let embedded_person = query.text.find("person").expect("person present");
        let host = query.host_offset(embedded_person);
        assert_eq!(&source[host..host + 6], "person");

        // `name` after the substitution maps verbatim too.
        let embedded_name = query.text.find("name =").expect("name present");
        let host = query.host_offset(embedded_name);
        assert_eq!(&source[host..host + 4], "name");

        // An offset inside the generated parameter maps to the `${`.
        let embedded_param = query.text.find("$__host0").expect("param present");
        let host = query.host_offset(embedded_param + 3);
        assert_eq!(&source[host..host + 2], "${");
    }

    #[test]
    fn call_form_string_literals_extract() {
        let source = "const q = surql(\"SELECT name FROM person WHERE team = $team\");";
        let queries = extract_typescript(source, false);

        assert_eq!(queries.len(), 1);
        assert_eq!(
            queries[0].text,
            "SELECT name FROM person WHERE team = $team"
        );
        assert!(queries[0].substitutions.is_empty());
        let embedded_team = queries[0].text.find("team =").expect("team present");
        let host = queries[0].host_offset(embedded_team);
        assert_eq!(&source[host..host + 4], "team");
    }

    #[test]
    fn db_query_string_literal_extracts() {
        let source =
            "const [rows] = await db.query(\"SELECT name FROM person WHERE team = $team\", { team });";
        let queries = extract_typescript(source, false);

        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].text, "SELECT name FROM person WHERE team = $team");
        assert!(queries[0].substitutions.is_empty());
        // The query maps back to the host string, past the bindings object.
        let embedded_team = queries[0].text.find("team =").expect("team present");
        let host = queries[0].host_offset(embedded_team);
        assert_eq!(&source[host..host + 4], "team");
    }

    #[test]
    fn db_live_string_literal_extracts() {
        let source = "const handle = db.live(\"LIVE SELECT * FROM person\");";
        let queries = extract_typescript(source, false);

        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].text, "LIVE SELECT * FROM person");
    }

    #[test]
    fn unrelated_query_method_with_options_object_is_ignored() {
        // React-Query-style `.query({...})` passes an options object, never a
        // string literal — it must not be picked up as a SurrealQL query.
        let source = "const r = client.query({ url: '/api', method: 'GET' });";
        let queries = extract_typescript(source, false);

        assert!(queries.is_empty());
    }

    #[test]
    fn member_tags_and_tsx_sources_work() {
        let source = "export const App = () => <div>{db.surql`SELECT 1 FROM person`}</div>;";
        let queries = extract_typescript(source, true);

        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].text, "SELECT 1 FROM person");
    }
}
