//! Embedded SurrealQL extraction from host-language sources.
//!
//! Host adapters share one convention: SurrealQL lives in `surql`
//! tagged-template literals (`` surql`SELECT * FROM person` ``). This
//! crate finds those templates with the host language's own grammar,
//! rewrites `${...}` substitutions into analyzer-visible parameters, and
//! keeps a byte-precise map from the extracted query back to the host
//! file — so findings computed on the query render at the right host
//! spans.
//!
//! Framework files (Svelte, Vue, Astro) are TypeScript inside
//! `<script>` blocks; [`extract`] handles both plain and framework
//! sources by extension.

mod typescript;

pub use typescript::extract_typescript;

/// One SurrealQL query found in a host file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedQuery {
    /// The query text as the analyzer should see it: template
    /// substitutions replaced by `$__hostN` parameters.
    pub text: String,
    /// Byte range of the template's contents in the host file.
    pub host_range: std::ops::Range<usize>,
    /// Copied-chunk map from `text` offsets to host offsets.
    segments: Vec<Segment>,
    /// The substitutions that became parameters, in order: the parameter
    /// name and the host byte range of the `${...}` expression.
    pub substitutions: Vec<Substitution>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Substitution {
    pub param: String,
    pub host_range: std::ops::Range<usize>,
}

/// A run of bytes copied verbatim from the host file into the query.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Segment {
    embed_start: usize,
    host_start: usize,
    len: usize,
}

impl EmbeddedQuery {
    /// Maps a byte offset in the extracted query text to its host-file
    /// offset. Offsets inside a substitution parameter map to the start
    /// of the host `${...}` expression.
    pub fn host_offset(&self, embed_offset: usize) -> usize {
        let position = self
            .segments
            .partition_point(|segment| segment.embed_start <= embed_offset);
        let Some(segment) = position.checked_sub(1).and_then(|i| self.segments.get(i)) else {
            return self.host_range.start;
        };
        let into = embed_offset - segment.embed_start;
        if into < segment.len {
            segment.host_start + into
        } else {
            // Between segments: inside a rewritten substitution. Its host
            // range starts right after this copied run.
            segment.host_start + segment.len
        }
    }

    /// Maps an embedded byte range to the smallest host range covering it.
    pub fn host_span(&self, range: std::ops::Range<usize>) -> std::ops::Range<usize> {
        let start = self.host_offset(range.start);
        let end = self
            .host_offset(range.end.max(range.start + 1).saturating_sub(1))
            .saturating_add(1)
            .max(start + 1);
        start..end
    }
}

/// Extracts embedded queries from a host source, dispatching on the file
/// extension: framework files are unwrapped to their `<script>` blocks
/// first, everything else parses as TypeScript/TSX directly.
pub fn extract(file_name: &str, text: &str) -> Vec<EmbeddedQuery> {
    let extension = file_name.rsplit('.').next().unwrap_or_default();
    match extension {
        "svelte" | "vue" | "astro" | "html" => script_blocks(text)
            .into_iter()
            .flat_map(|block| {
                let mut queries = extract_typescript(&text[block.clone()], true);
                for query in &mut queries {
                    shift(query, block.start);
                }
                queries
            })
            .collect(),
        "tsx" | "jsx" => extract_typescript(text, true),
        _ => extract_typescript(text, false),
    }
}

fn shift(query: &mut EmbeddedQuery, by: usize) {
    query.host_range = query.host_range.start + by..query.host_range.end + by;
    for segment in &mut query.segments {
        segment.host_start += by;
    }
    for substitution in &mut query.substitutions {
        substitution.host_range =
            substitution.host_range.start + by..substitution.host_range.end + by;
    }
}

/// The content ranges of `<script ...>...</script>` blocks.
fn script_blocks(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut blocks = Vec::new();
    let lower = text.to_ascii_lowercase();
    let mut from = 0;
    while let Some(open_at) = lower[from..].find("<script") {
        let open_at = from + open_at;
        let Some(open_end) = lower[open_at..].find('>') else {
            break;
        };
        let content_start = open_at + open_end + 1;
        let Some(close_at) = lower[content_start..].find("</script") else {
            break;
        };
        blocks.push(content_start..content_start + close_at);
        from = content_start + close_at + 1;
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svelte_script_blocks_extract_with_shifted_offsets() {
        let source = "<h1>hi</h1>\n<script lang=\"ts\">\nconst q = surql`SELECT * FROM person`;\n</script>\n";
        let queries = extract("app.svelte", source);

        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].text, "SELECT * FROM person");
        let host = &source[queries[0].host_range.clone()];
        assert_eq!(host, "SELECT * FROM person");
        // `person` starts at embedded offset 14; the host offset must
        // point at the same word in the full file.
        let mapped = queries[0].host_offset(14);
        assert_eq!(&source[mapped..mapped + 6], "person");
    }
}
