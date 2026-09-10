//! Embedded-query extraction from Svelte **markup**.
//!
//! A Svelte app runs queries from two places. One is a `<script>` block,
//! which is TypeScript and which [`crate::extract_typescript`] already
//! handles. The other is the markup itself, on the components that run a
//! query declaratively:
//!
//! ```svelte
//! <Query q="SELECT * FROM user" />
//! <LiveQuery q="SELECT * FROM user WHERE age > {minAge}" />
//! ```
//!
//! Those attributes are real queries against the real schema, so a bad
//! field in one is a bug the same as a bad field in `db.query(...)`. Until
//! this module existed they were simply invisible.
//!
//! # Why the grammar, and not a regex
//!
//! Attribute values are not scannable with a pattern. `>` is legal inside a
//! quoted attribute — `q="… WHERE age > 18"` is the *common* case, not an
//! exotic one — so a regex that ends a tag at the first `>` truncates
//! exactly the queries we care about most. Interpolations nest and can
//! themselves contain braces and quoted strings: `{a.b['}']}` ends at the
//! last `}`, not the first. And `<Query>` written inside a `<script>` string
//! is not markup at all.
//!
//! The Svelte grammar decides all three correctly, and hands back byte
//! ranges, which is what the span map needs anyway.

use tree_sitter::{Node, Parser};

use crate::{EmbeddedQuery, Segment, Substitution, HOST_PARAM_PREFIX};

/// The elements whose query attribute is scanned, and whether the element
/// runs its query as a **live** query.
///
/// Deliberately a closed set. Scanning every attribute of every component
/// would hand arbitrary strings — CSS classes, labels, URLs — to a SurrealQL
/// parser and report syntax errors on all of them; the noise would bury the
/// findings that matter and train the reader to ignore the tool. These two
/// names are the components that exist to run a query, so their `q` is
/// SurrealQL by construction.
pub const QUERY_ELEMENTS: [(&str, bool); 2] = [("Query", false), ("LiveQuery", true)];

/// The one attribute read on a [`QUERY_ELEMENTS`] element.
pub const QUERY_ATTRIBUTE: &str = "q";

/// Finds every SurrealQL query embedded in a Svelte source's markup
/// attributes. Offsets are already in whole-file coordinates — the grammar
/// parses the file, not a fragment of it — so callers do not shift them.
#[must_use]
pub fn extract_svelte_markup(text: &str) -> Vec<EmbeddedQuery> {
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_svelte_ng::LANGUAGE.into())
        .is_err()
    {
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
    if matches!(node.kind(), "start_tag" | "self_closing_tag") {
        if let Some(live) = element_live_flag(node, text) {
            if let Some(value) = query_attribute_value(node, text) {
                if let Some(mut query) = attribute_value_to_query(value, text) {
                    query.live = live;
                    queries.push(query);
                }
            }
        }
    }
    let mut walker = node.walk();
    let children: Vec<_> = node.children(&mut walker).collect();
    for child in children {
        collect(child, text, queries);
    }
}

/// `Some(live)` when the tag names a [`QUERY_ELEMENTS`] element.
///
/// The match is case-sensitive: Svelte itself distinguishes a component
/// (capitalised) from an HTML element by case, so a lowercase `<query>` is a
/// plain unknown element and not the component we mean.
fn element_live_flag(tag: Node<'_>, text: &str) -> Option<bool> {
    let mut walker = tag.walk();
    let name = tag
        .children(&mut walker)
        .find(|child| child.kind() == "tag_name")?;
    let name = &text[name.byte_range()];
    QUERY_ELEMENTS
        .iter()
        .find_map(|(element, live)| (*element == name).then_some(*live))
}

/// The `attribute_value` node of the tag's `q` attribute, if it has a static
/// one.
///
/// `q={someVariable}` deliberately yields nothing: the whole value is a
/// runtime expression, so there is no query text to analyse and no honest
/// finding to raise about it.
fn query_attribute_value<'tree>(tag: Node<'tree>, text: &str) -> Option<Node<'tree>> {
    let mut walker = tag.walk();
    let attributes: Vec<_> = tag
        .children(&mut walker)
        .filter(|child| child.kind() == "attribute")
        .collect();

    for attribute in attributes {
        let mut walker = attribute.walk();
        let children: Vec<_> = attribute.children(&mut walker).collect();
        let named = children
            .iter()
            .find(|child| child.kind() == "attribute_name")
            .is_some_and(|name| &text[name.byte_range()] == QUERY_ATTRIBUTE);
        if !named {
            continue;
        }
        // Quoted (`q="…"` / `q='…'`) or bare (`q=SELECT`). A `quoted_attribute_value`
        // with no `attribute_value` child is an empty string — nothing to analyse.
        return children.iter().find_map(|child| match child.kind() {
            "attribute_value" => Some(*child),
            "quoted_attribute_value" => {
                let mut walker = child.walk();
                let inner = child
                    .children(&mut walker)
                    .find(|inner| inner.kind() == "attribute_value");
                inner
            }
            _ => None,
        });
    }
    None
}

/// Rebuilds an attribute value as analyzable SurrealQL: literal text copies
/// verbatim (one segment-map entry each), and every `{...}` interpolation
/// becomes a `$__hostN` parameter.
///
/// The parameter is what makes the skeleton parse at all. `WHERE age >
/// {minAge}` is not SurrealQL — the analyzer would report a syntax error at
/// the brace and never reach the field it was asked about. Rewriting the
/// hole to a parameter reference gives it a well-formed query whose every
/// other token still maps back to real host bytes, so the finding the user
/// actually needs — the bad field two words earlier — comes out at the right
/// place.
fn attribute_value_to_query(value: Node<'_>, text: &str) -> Option<EmbeddedQuery> {
    let content = value.byte_range();
    if content.is_empty() {
        return None;
    }

    let mut query = String::new();
    let mut segments = Vec::new();
    let mut substitutions = Vec::new();
    let mut host_cursor = content.start;

    let mut walker = value.walk();
    let children: Vec<_> = value.children(&mut walker).collect();
    for child in children {
        if child.kind() != "expression" {
            continue;
        }
        push_fragment(
            text,
            host_cursor..child.start_byte(),
            &mut query,
            &mut segments,
        );
        let param = format!("{HOST_PARAM_PREFIX}{}", substitutions.len());
        let embed_start = query.len();
        query.push('$');
        query.push_str(&param);
        substitutions.push(Substitution {
            param,
            host_range: child.byte_range(),
            embed_range: embed_start..query.len(),
        });
        host_cursor = child.end_byte();
    }
    push_fragment(text, host_cursor..content.end, &mut query, &mut segments);

    Some(EmbeddedQuery {
        text: query,
        host_range: content,
        segments,
        substitutions,
        live: false,
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
    fn a_plain_attribute_query_extracts_verbatim() {
        let source = "<Query q=\"SELECT * FROM user\" />";
        let queries = extract_svelte_markup(source);

        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].text, "SELECT * FROM user");
        assert!(queries[0].substitutions.is_empty());
        assert!(!queries[0].live);
        assert_eq!(&source[queries[0].host_range.clone()], "SELECT * FROM user");
    }

    #[test]
    fn interpolations_become_host_parameters() {
        let source = "<Query q=\"SELECT * FROM user WHERE age > {minAge} AND team = {team}\" />";
        let queries = extract_svelte_markup(source);

        assert_eq!(queries.len(), 1);
        assert_eq!(
            queries[0].text,
            "SELECT * FROM user WHERE age > $__host0 AND team = $__host1"
        );
        assert_eq!(
            &source[queries[0].substitutions[0].host_range.clone()],
            "{minAge}"
        );
        assert_eq!(
            &source[queries[0].substitutions[1].host_range.clone()],
            "{team}"
        );
    }

    #[test]
    fn a_live_element_marks_its_query_live() {
        let queries = extract_svelte_markup("<LiveQuery q=\"SELECT * FROM user\" />");

        assert_eq!(queries.len(), 1);
        assert!(queries[0].live);
    }

    #[test]
    fn only_the_query_elements_and_the_q_attribute_are_scanned() {
        // An arbitrary component's arbitrary attribute is not SurrealQL, and
        // parsing it as such would report a syntax error on someone's CSS.
        let source = r#"
<div class="SELECT * FROM user" />
<Card title="not a query" q="also not scanned because Card is not a query element" />
<Query class="SELECT bogus" label="nope" />
"#;
        assert!(extract_svelte_markup(source).is_empty());
    }

    #[test]
    fn a_fully_dynamic_attribute_yields_nothing() {
        // `q={expr}` has no query text at all — there is nothing true to say
        // about it, so saying nothing is the honest answer.
        assert!(extract_svelte_markup("<Query q={dynamic} />").is_empty());
        assert!(extract_svelte_markup("<Query q=\"\" />").is_empty());
    }

    #[test]
    fn a_greater_than_inside_the_value_does_not_end_the_tag() {
        // The regex failure this module exists to avoid: `>` is legal inside
        // a quoted attribute, and it is exactly what a comparison looks like.
        let source = "<Query q=\"SELECT * FROM user WHERE age > 18 AND rank < 3\" />";
        let queries = extract_svelte_markup(source);

        assert_eq!(queries.len(), 1);
        assert_eq!(
            queries[0].text,
            "SELECT * FROM user WHERE age > 18 AND rank < 3"
        );
    }

    #[test]
    fn a_brace_inside_an_interpolation_string_does_not_end_the_hole() {
        // The second regex failure: the hole ends at the matching brace, not
        // at the one sitting inside a JS string literal.
        let source = "<Query q=\"SELECT * FROM user WHERE tag = {map['}']}\" />";
        let queries = extract_svelte_markup(source);

        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].text, "SELECT * FROM user WHERE tag = $__host0");
        assert_eq!(
            &source[queries[0].substitutions[0].host_range.clone()],
            "{map['}']}"
        );
    }

    #[test]
    fn single_quoted_attributes_and_nested_elements_are_found() {
        let source = r#"
{#if ready}
  <div>
    <Query q='SELECT name FROM user' />
  </div>
{/if}
"#;
        let queries = extract_svelte_markup(source);

        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].text, "SELECT name FROM user");
    }

    #[test]
    fn a_commented_out_element_is_not_scanned() {
        // Commenting a component out is how a developer disables it. Still
        // reporting on it would make the finding unfixable by the one edit
        // that should have silenced it.
        let source = "<!-- <Query q=\"SELECT * FROM commentedout\" /> -->\n";
        assert!(extract_svelte_markup(source).is_empty());
    }

    #[test]
    fn each_blocks_and_rune_syntax_do_not_confuse_the_scan() {
        let source = r#"
<script lang="ts">
  let items = $state([1, 2, 3]);
</script>

{#each items as item}
  <Query q="SELECT * FROM user WHERE age > {item}" />
{/each}
"#;
        let queries = extract_svelte_markup(source);

        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].text, "SELECT * FROM user WHERE age > $__host0");
        assert_eq!(
            &source[queries[0].substitutions[0].host_range.clone()],
            "{item}"
        );
    }

    #[test]
    fn markup_inside_a_script_string_is_not_markup() {
        // A `<Query>` spelled inside script text is a string, not an element.
        let source =
            "<script>\n  const s = '<Query q=\"SELECT bogus FROM nowhere\" />';\n</script>\n";
        assert!(extract_svelte_markup(source).is_empty());
    }
}
