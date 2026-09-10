//! Embedded SurrealQL extraction from host-language sources.
//!
//! Host adapters share one convention: SurrealQL lives in a **string
//! literal** passed to a query sink — `db.query("SELECT * FROM person")`,
//! `defineQuery("...")`, `defineLive("...")`. This crate finds those calls
//! with the host language's own grammar, rewrites the `${...}`
//! substitutions of a template argument into analyzer-visible parameters,
//! and keeps a byte-precise map from the extracted query back to the host
//! file — so findings computed on the query render at the right host
//! spans.
//!
//! A string literal rather than a tagged template because only a literal
//! survives into the type system: `TemplateStringsArray` has no generic
//! parameter, so `` surql`…` `` erases its query before inference can look
//! at it. Extraction and the generated registry key on the same text, which
//! only the literal form makes possible.
//!
//! Framework files (Svelte, Vue, Astro) are TypeScript inside
//! `<script>` blocks; [`extract`] handles both plain and framework
//! sources by extension.
//!
//! Svelte files carry queries in a second place: **markup attributes**, on
//! the components that run them — `<Query q="SELECT …">`. Those are found
//! by [`svelte::extract_svelte_markup`] over the Svelte grammar, and they
//! produce the same [`EmbeddedQuery`] with the same span map, so everything
//! downstream treats a markup query exactly like a script one.

mod svelte;
mod typescript;

pub use svelte::{extract_svelte_markup, QUERY_ATTRIBUTE, QUERY_ELEMENTS};
pub use typescript::extract_typescript;

/// The prefix of the parameter names generated for host substitutions:
/// `${...}` in a TypeScript template, `{...}` in a Svelte markup attribute.
///
/// Both extraction paths share it so the two cannot drift: a substitution is
/// a *host* substitution whichever syntax spelled it, and the generated
/// registry and the framework runtime bind these names by convention. The
/// leading underscores are the point — `__host0` is reserved-looking, so it
/// cannot collide with a parameter the user wrote themselves, the way a
/// short name like `p0` silently would.
pub const HOST_PARAM_PREFIX: &str = "__host";

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
    /// Whether the sink that received this query runs it as a **live query**
    /// — `defineLive`. The text is an ordinary SELECT and analyzes as one, but
    /// the client wraps it in `LIVE SELECT`, which is a far narrower statement
    /// than SELECT. Without this the analyzer cannot tell the two apart, and
    /// `defineLive("SELECT * FROM user:1")` passes as a perfectly good query
    /// while being a subscription that never fires.
    pub live: bool,
}

/// A host substitution rewritten into an analyzer parameter — `${...}` in a
/// TypeScript template, `{...}` in a Svelte markup attribute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Substitution {
    /// The generated parameter name (`__hostN`) standing in for the
    /// substitution.
    pub param: String,
    /// Byte range of the original host expression — `${min}` / `{minAge}`,
    /// braces included.
    pub host_range: std::ops::Range<usize>,
    /// Byte range of the generated `$__hostN` text inside [`EmbeddedQuery::text`].
    ///
    /// This is what lets a finding raised *on the parameter* be reported
    /// against the whole host expression the user actually wrote, rather
    /// than against the single byte the copied-run map would otherwise
    /// resolve to.
    pub embed_range: std::ops::Range<usize>,
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

    /// Maps a host-file byte offset back into the extracted query text —
    /// the inverse of [`Self::host_offset`]. `None` when the offset is not
    /// inside a copied run: outside the template entirely, or inside a
    /// `${...}` substitution, neither of which names a position in the
    /// query the analyzer saw.
    ///
    /// This is what lets a cursor-addressed request (hover, completion)
    /// asked at a host position be answered by the query's own analysis.
    pub fn embed_offset(&self, host_offset: usize) -> Option<usize> {
        self.segments.iter().find_map(|segment| {
            let into = host_offset.checked_sub(segment.host_start)?;
            (into < segment.len).then_some(segment.embed_start + into)
        })
    }

    /// The template's static parts in order — the copied text runs
    /// between substitutions. One part for substitution-free queries.
    pub fn parts(&self) -> Vec<String> {
        if self.substitutions.is_empty() {
            return vec![self.text.clone()];
        }
        let mut parts = Vec::new();
        let mut cursor = 0;
        for substitution in &self.substitutions {
            let marker = format!("${}", substitution.param);
            let at = self.text[cursor..]
                .find(&marker)
                .map_or(self.text.len(), |i| cursor + i);
            parts.push(self.text[cursor..at].to_string());
            cursor = (at + marker.len()).min(self.text.len());
        }
        parts.push(self.text[cursor..].to_string());
        parts
    }

    /// Maps an embedded byte range to the smallest host range covering it.
    ///
    /// A range that touches a generated `$__hostN` parameter widens to cover
    /// that substitution's whole host expression. Without this, a finding on
    /// the parameter collapses onto the one byte the copied-run map resolves
    /// to — an opening `{` — and the reader gets a caret under punctuation
    /// instead of under the `{minAge}` they wrote.
    pub fn host_span(&self, range: std::ops::Range<usize>) -> std::ops::Range<usize> {
        let end_of = range.end.max(range.start + 1);
        let start = self.host_offset(range.start);
        let end = self
            .host_offset(end_of.saturating_sub(1))
            .saturating_add(1)
            .max(start + 1);

        let mut span = start..end;
        for substitution in &self.substitutions {
            let overlaps = substitution.embed_range.start < end_of
                && range.start < substitution.embed_range.end;
            if overlaps {
                span.start = span.start.min(substitution.host_range.start);
                span.end = span.end.max(substitution.host_range.end);
            }
        }
        span
    }
}

/// Extracts embedded queries from a host source, dispatching on the file
/// extension: framework files are unwrapped to their `<script>` blocks
/// first, everything else parses as TypeScript/TSX directly.
pub fn extract(file_name: &str, text: &str) -> Vec<EmbeddedQuery> {
    let extension = file_name.rsplit('.').next().unwrap_or_default();
    match extension {
        "svelte" | "vue" | "astro" | "html" => {
            let mut queries: Vec<EmbeddedQuery> = script_blocks(text)
                .into_iter()
                .flat_map(|block| {
                    let mut queries = extract_typescript(&text[block.clone()], true);
                    for query in &mut queries {
                        shift(query, block.start);
                    }
                    queries
                })
                .collect();

            // Markup attributes are Svelte's alone: `{expr}` in an attribute
            // is an interpolation there, and something else (or nothing) in
            // Vue, Astro and plain HTML. Scanning those with the Svelte
            // grammar would invent substitutions their syntax never had.
            if extension == "svelte" {
                queries.extend(extract_svelte_markup(text));
            }

            // Document order, so the `embedded://host#N` ids the CLI and the
            // LSP mint stay in reading order once two extractors contribute.
            queries.sort_by_key(|query| (query.host_range.start, query.host_range.end));
            queries
        }
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
        let source = "<h1>hi</h1>\n<script lang=\"ts\">\nconst q = db.query(\"SELECT * FROM person\");\n</script>\n";
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

    #[test]
    fn host_offsets_map_back_into_the_query() {
        let source = "const q = db.query(`SELECT * FROM person WHERE age > ${min}`);";
        let queries = extract("app.ts", source);
        let query = &queries[0];

        // Every copied byte round-trips: a host offset inside the template
        // names the same byte of the query text.
        let person = source.find("person").expect("person present");
        let embedded = query.embed_offset(person).expect("inside a copied run");
        assert_eq!(&query.text[embedded..embedded + 6], "person");
        assert_eq!(query.host_offset(embedded), person);

        // A `${...}` substitution is not a position in the query: the
        // analyzer saw `$__host0` there, and pointing at the middle of that
        // generated name would answer about text the user never wrote.
        let substitution = source.find("${min}").expect("substitution present");
        assert_eq!(query.embed_offset(substitution + 2), None);

        // Outside the template entirely.
        assert_eq!(query.embed_offset(0), None);
    }
}
