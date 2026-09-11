//! Host-file analysis: everything a TypeScript language service asks about a
//! `.ts` / `.svelte` file, answered in one call.
//!
//! The playground's [`crate::analyze`] takes a query and answers about the
//! query. An editor takes a *host file* and asks three different questions
//! about it on a keystroke cadence — what is wrong, what colour is each byte,
//! what is the type under the cursor — and every one of them needs the same
//! two things first: the embedded queries, and an analysis of each. Doing that
//! extraction and analysis once per call, rather than once per question, is
//! what makes the plugin fast enough to sit inline in `getSemanticDiagnostics`.
//!
//! Everything crossing the boundary is in **host-file byte offsets**. The
//! caller has a `.svelte` file and a cursor in it; it must never have to know
//! that we rewrote `${…}` into `$__host0` to analyze the query.
//!
//! # The catalog is cached inside the module, not by the caller
//!
//! Building the schema catalog is the expensive half — it parses and walks
//! every `.surql` source. The host file changes on every keystroke; the schema
//! changes when someone saves a migration. So the module holds the last
//! catalog keyed by a hash of the schema text and rebuilds only when that
//! moves. The caller still passes the whole schema every call: it is a memcpy
//! of a few dozen kilobytes against an analysis that would otherwise cost
//! milliseconds, and it keeps the ABI stateless from the outside — there is no
//! handle to leak and no way for the host to hold a catalog that no longer
//! matches the files on disk.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use surrealql_analyzer_diagnostics::{Finding, FindingTag, Severity};
use surrealql_analyzer_embed::EmbeddedQuery;
use surrealql_analyzer_syntax::highlight;
use surrealql_analyzer_syntax::parse::{parse_source, ParsedSource};
use surrealql_analyzer_syntax::source::SourceId;
use surrealql_analyzer_syntax::span::{ByteRange, SourceSpan};
use surrealql_analyzer_workspace::analysis::{
    analyze_one_source, analyze_workspace, build_global_catalog, build_workspace_schema,
    AnalysisOutput, GlobalCatalog, Workspace,
};
use surrealql_analyzer_workspace::schema::SchemaIndex;

/// What the host asks about one file.
#[derive(Deserialize)]
pub struct Request {
    /// Every `.surql` source the workspace has, concatenated. Empty when the
    /// project has no schema.
    #[serde(default)]
    pub schema: String,
    /// The host file's name. Only the extension is read, to pick the grammar
    /// and to decide whether `<script>` blocks need unwrapping first.
    #[serde(default)]
    pub file_name: String,
    /// The host file's full current text.
    #[serde(default)]
    pub source: String,
    /// A host byte offset to answer a hover for, when the caller wants one.
    #[serde(default)]
    pub hover: Option<u32>,
}

/// One finding, at host-file byte offsets.
///
/// The attachments are additive fields, omitted from the JSON when empty: a
/// consumer that only reads `code`/`severity`/`message`/`start`/`end` sees
/// exactly what it saw before.
#[derive(Serialize)]
pub struct Diagnostic {
    /// Stable finding code, e.g. `"E1002"`.
    pub code: String,
    /// `"error"`, `"warning"`, or `"hint"`.
    pub severity: &'static str,
    /// Human-readable message.
    pub message: String,
    /// Byte offset of the span start in the host file.
    pub start: u32,
    /// Byte offset of the span end in the host file.
    pub end: u32,
    /// Actionable suggestions, in the order the analyzer attached them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub help: Vec<Help>,
    /// Secondary locations explaining the finding — *only* the ones that land
    /// in this host file. A related span pointing into the schema is dropped:
    /// the schema reaches this module as one concatenated string with no file
    /// map, so there is no location the caller could resolve it to.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<Related>,
    /// Rendering hints: `"unnecessary"` (fade) and `"deprecated"` (strike).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<&'static str>,
}

/// An actionable suggestion attached to a finding.
#[derive(Serialize)]
pub struct Help {
    /// The suggestion text.
    pub message: String,
    /// Replacement text for the finding's span, when the fix is mechanical.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
}

/// A secondary location that explains a finding, at host-file byte offsets.
#[derive(Serialize)]
pub struct Related {
    /// What this location contributes to the finding.
    pub message: String,
    /// Byte offset of the span start in the host file.
    pub start: u32,
    /// Byte offset of the span end in the host file.
    pub end: u32,
}

/// One highlighting token, at host-file byte offsets.
#[derive(Serialize)]
pub struct Token {
    /// Byte offset of the token start in the host file.
    pub start: u32,
    /// Byte offset of the token end in the host file.
    pub end: u32,
    /// The token's kind, as its index in the shared legend
    /// ([`highlight::TokenKind::index`]).
    pub kind: u32,
}

/// One embedded query's extent in the host file.
#[derive(Serialize)]
pub struct Query {
    /// Byte offset of the query text's start in the host file.
    pub start: u32,
    /// Byte offset of the query text's end in the host file.
    pub end: u32,
}

/// The type under the cursor, at host-file byte offsets.
#[derive(Serialize)]
pub struct Hover {
    /// Markdown, the same text the LSP would show.
    pub markdown: String,
    /// Byte offset of the covered token's start in the host file.
    pub start: u32,
    /// Byte offset of the covered token's end in the host file.
    pub end: u32,
}

/// Everything known about one host file.
#[derive(Serialize)]
pub struct Response {
    /// Findings from every embedded query, re-spanned onto the host.
    pub diagnostics: Vec<Diagnostic>,
    /// Highlighting tokens inside the embedded queries only.
    pub tokens: Vec<Token>,
    /// Where each embedded query sits, so the caller can tell "inside a query"
    /// from "ordinary TypeScript" without re-extracting.
    pub queries: Vec<Query>,
    /// The hover answer, when one was asked for and there was something typed
    /// under the cursor.
    pub hover: Option<Hover>,
}

impl Response {
    /// The answer for a file with nothing embedded in it: three empty lists
    /// and no hover. Distinct from an error — there is genuinely nothing to
    /// say, and saying it costs the caller nothing.
    fn empty() -> Self {
        Self {
            diagnostics: Vec::new(),
            tokens: Vec::new(),
            queries: Vec::new(),
            hover: None,
        }
    }
}

/// The schema, parsed and indexed, reusable across host files and keystrokes.
struct Catalog {
    /// Hash of the schema text this was built from.
    key: u64,
    /// The cross-source catalog a single query is analyzed against.
    global: GlobalCatalog,
    /// The schema index hover renders declared types out of.
    schema: SchemaIndex,
}

static CATALOG: Mutex<Option<Catalog>> = Mutex::new(None);

fn hash(text: &str) -> u64 {
    // FNV-1a: no dependency, no HashMap seeding, and stable across calls —
    // `DefaultHasher` is explicitly not guaranteed to be, and a key that
    // changes for identical input would rebuild the catalog every keystroke.
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

/// Runs `f` against the catalog for `schema`, building it only when the
/// schema text differs from the cached one.
fn with_catalog<T>(schema: &str, f: impl FnOnce(&Catalog) -> T) -> T {
    let key = hash(schema);
    let mut guard = CATALOG.lock().unwrap_or_else(|poisoned| {
        // A panic in the analyzer must not turn every later request into a
        // panic of its own; the cache is a cache.
        CATALOG.clear_poison();
        poisoned.into_inner()
    });
    if guard.as_ref().is_none_or(|catalog| catalog.key != key) {
        let parsed = parse_source(SourceId::new("schema"), schema)
            .ok()
            .map(|parsed| vec![parsed])
            .unwrap_or_default();
        *guard = Some(Catalog {
            key,
            global: build_global_catalog(&parsed),
            schema: build_workspace_schema(&parsed),
        });
    }
    f(guard.as_ref().expect("just populated"))
}

/// One embedded query with its analysis, in embedded coordinates.
struct Analyzed {
    query: EmbeddedQuery,
    parsed: ParsedSource,
    output: AnalysisOutput,
    source: SourceId,
}

/// Analyzes a host file and answers every question in `request` from the one
/// pass.
///
/// Returns the JSON encoding of a [`Response`]. Errors are not a shape the
/// caller has to handle: a file that does not parse, a schema that does not
/// parse, and a file with no queries in it all produce an empty response,
/// because in an editor there is no difference between "we could not tell you
/// anything" and "there was nothing to tell". A plugin that reported its own
/// failures inside someone's TypeScript file would be noise, not a diagnostic.
#[must_use]
pub fn analyze_host(request: &Request) -> String {
    let response = run(request);
    serde_json::to_string(&response).unwrap_or_else(|_| {
        r#"{"diagnostics":[],"tokens":[],"queries":[],"hover":null}"#.to_string()
    })
}

fn run(request: &Request) -> Response {
    let queries = surrealql_analyzer_embed::extract(&request.file_name, &request.source);
    if queries.is_empty() {
        return Response::empty();
    }

    let analyzed = analyze(&request.schema, queries);
    let mut response = Response {
        diagnostics: Vec::new(),
        tokens: Vec::new(),
        queries: analyzed
            .iter()
            .map(|entry| Query {
                start: entry.query.host_range.start as u32,
                end: entry.query.host_range.end as u32,
            })
            .collect(),
        hover: None,
    };

    let host_source = SourceId::new("host");
    for entry in &analyzed {
        for finding in &entry.output.diagnostics {
            response
                .diagnostics
                .push(to_host_diagnostic(finding, &analyzed, &host_source));
        }
        response
            .tokens
            .extend(host_tokens(&entry.query, &entry.parsed));
    }

    // Diagnostics and tokens must be in ascending host order: the caller feeds
    // both to TypeScript, whose classification encoding is positional.
    response.diagnostics.sort_by_key(|d| (d.start, d.end));
    response.tokens.sort_by_key(|t| (t.start, t.end));

    if let Some(offset) = request.hover {
        response.hover = hover(&request.schema, &analyzed, offset);
    }
    response
}

/// Every token of a query, re-expressed as ranges in its host file.
///
/// A token is kept only when it maps to a contiguous run of host bytes. One
/// that straddles a `${...}` substitution does not: it covers a generated
/// `$__hostN` name on our side and an arbitrary host expression on the
/// editor's, and painting it would colour the wrong text.
fn host_tokens(query: &EmbeddedQuery, parsed: &ParsedSource) -> Vec<Token> {
    highlight::tokens(parsed)
        .into_iter()
        .filter_map(|token| {
            let length = token.range.end - token.range.start;
            let host = query.host_span(token.range);
            (host.end - host.start == length).then_some(Token {
                start: host.start as u32,
                end: host.end as u32,
                kind: token.kind.index(),
            })
        })
        .collect()
}

/// Analyzes every extracted query against the schema.
///
/// Read-only queries — the overwhelming majority of what a client file holds —
/// each analyze alone against the cached catalog, so a keystroke costs one
/// small query rather than a whole-workspace pass. A query that could *change*
/// the catalog (a `CREATE`, a `DEFINE`) breaks [`analyze_one_source`]'s
/// contract, so those fall back to a full pass with the schema and the queries
/// registered together.
fn analyze(schema: &str, queries: Vec<EmbeddedQuery>) -> Vec<Analyzed> {
    let source_id = |index: usize| SourceId::new(format!("embedded://host#{index}"));

    if queries.iter().any(|query| is_schema_relevant(&query.text)) {
        let mut workspace = Workspace::default();
        if !schema.trim().is_empty() {
            workspace.add_virtual_source("schema".into(), schema.to_string());
        }
        let registered: Vec<_> = queries
            .into_iter()
            .enumerate()
            .map(|(index, query)| {
                let source =
                    workspace.add_virtual_source(source_id(index).to_string(), query.text.clone());
                (source, query)
            })
            .collect();
        let analysis = analyze_workspace(&workspace);
        return registered
            .into_iter()
            .filter_map(|(source, query)| {
                let parsed = parse_source(source.clone(), query.text.as_str()).ok()?;
                let output = analysis.sources.get(&source)?.clone();
                Some(Analyzed {
                    query,
                    parsed,
                    output,
                    source,
                })
            })
            .collect();
    }

    with_catalog(schema, |catalog| {
        queries
            .into_iter()
            .enumerate()
            .filter_map(|(index, query)| {
                let source = source_id(index);
                let parsed = parse_source(source.clone(), query.text.as_str()).ok()?;
                let output = analyze_one_source(&catalog.global, &parsed, false);
                Some(Analyzed {
                    query,
                    parsed,
                    output,
                    source,
                })
            })
            .collect()
    })
}

/// The hover answer at a host byte offset, or `None` when the offset is not
/// inside a query or names nothing typed.
fn hover(schema: &str, analyzed: &[Analyzed], host_offset: u32) -> Option<Hover> {
    let entry = analyzed
        .iter()
        .find(|entry| entry.query.embed_offset(host_offset as usize).is_some())?;
    let offset = entry.query.embed_offset(host_offset as usize)?;

    let render = |schema_index: &SchemaIndex| {
        surrealql_analyzer_workspace::hover_at(
            &entry.output,
            schema_index,
            &entry.source,
            &entry.query.text,
            offset as u32,
        )
    };
    // A schema-effecting file took the full-workspace path and never touched
    // the cached catalog; re-deriving the index here is the same work that
    // path already paid for and keeps the two paths answering alike.
    let info = with_catalog(schema, |catalog| render(&catalog.schema))?;

    let range = info.span.range();
    let host = entry
        .query
        .host_span(range.start() as usize..range.end() as usize);
    Some(Hover {
        markdown: info.markdown,
        start: host.start as u32,
        end: host.end as u32,
    })
}

/// Rebuilds a finding raised on an embedded query so every span it carries —
/// the primary one and each related location — points into the host file, then
/// flattens it for the caller.
///
/// The re-spanning is [`Finding::map_spans`], the same call the LSP and the
/// CLI make. Hand-rolling it here is how this surface used to end up dropping
/// `help`, `related` and `tags` that the other two showed.
///
/// A span belonging to *any* embedded query maps, not just this finding's own:
/// one query's finding can point at another's text, and both are in this file.
/// A span in the schema stays where it is and is then dropped, since the
/// caller has no schema file to point at.
fn to_host_diagnostic(
    finding: &Finding,
    analyzed: &[Analyzed],
    host_source: &SourceId,
) -> Diagnostic {
    let mapped = finding.map_spans(|span| {
        let Some(entry) = analyzed.iter().find(|entry| &entry.source == span.source()) else {
            return span.clone();
        };
        let range = span.range();
        let host = entry
            .query
            .host_span(range.start() as usize..range.end() as usize);
        SourceSpan::new(
            host_source.clone(),
            ByteRange::new(host.start as u32, host.end as u32)
                .expect("an embedded query's host span is ordered"),
        )
    });

    let range = mapped.span().range();
    Diagnostic {
        code: mapped.code().to_string(),
        severity: severity_label(mapped.severity()),
        message: mapped.message().to_string(),
        start: range.start(),
        end: range.end(),
        help: mapped
            .help()
            .iter()
            .map(|help| Help {
                message: help.message.clone(),
                replacement: help.replacement.clone(),
            })
            .collect(),
        related: mapped
            .related()
            .iter()
            .filter(|related| related.span.source() == host_source)
            .map(|related| Related {
                message: related.message.clone(),
                start: related.span.range().start(),
                end: related.span.range().end(),
            })
            .collect(),
        tags: mapped
            .tags()
            .iter()
            .map(|tag| match tag {
                FindingTag::Unnecessary => "unnecessary",
                FindingTag::Deprecated => "deprecated",
            })
            .collect(),
    }
}

fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Hint => "hint",
    }
}

/// Whether a query might contribute to the catalog — i.e. contains any
/// statement a *pure query* does not. Conservative: it whole-word-matches the
/// schema-affecting keywords, over-reporting (a keyword inside a string
/// literal takes the slow path) but never under-reporting.
fn is_schema_relevant(text: &str) -> bool {
    const KEYWORDS: [&str; 7] = [
        "define", "remove", "alter", "create", "upsert", "insert", "delete",
    ];
    let lower = text.to_ascii_lowercase();
    KEYWORDS
        .iter()
        .any(|keyword| contains_whole_word(&lower, keyword))
}

/// Whether `haystack` (already lowercased) contains `keyword` as a whole word,
/// so a field named `created_at` does not mark a query as `CREATE`-relevant.
fn contains_whole_word(haystack: &str, keyword: &str) -> bool {
    let bytes = haystack.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut from = 0;
    while let Some(pos) = haystack[from..].find(keyword) {
        let start = from + pos;
        let end = start + keyword.len();
        let before_ok = start == 0 || !is_ident(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_ident(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = "DEFINE TABLE account SCHEMAFULL;\n\
                          DEFINE FIELD username ON account TYPE string;\n";

    fn request(file_name: &str, source: &str) -> Request {
        Request {
            schema: SCHEMA.to_string(),
            file_name: file_name.to_string(),
            source: source.to_string(),
            hover: None,
        }
    }

    fn run_json(request: &Request) -> serde_json::Value {
        serde_json::from_str(&analyze_host(request)).expect("valid json")
    }

    #[test]
    fn a_finding_lands_on_the_host_bytes_it_is_about() {
        // The whole contract in one assertion: slice the *host* file at the
        // reported span and get the offending token. Anything else — the whole
        // string, the whole call, an off-by-the-quote — reads as a squiggle in
        // the wrong place, which is worse than no squiggle.
        let source = "const q = db.query(\"SELECT username, nope FROM account\");\n";
        let value = run_json(&request("app.ts", source));
        let diagnostics = value["diagnostics"].as_array().expect("array");
        let unknown = diagnostics
            .iter()
            .find(|d| d["code"] == "E1002")
            .unwrap_or_else(|| panic!("expected E1002, got {diagnostics:?}"));
        let start = unknown["start"].as_u64().expect("start") as usize;
        let end = unknown["end"].as_u64().expect("end") as usize;
        assert_eq!(&source[start..end], "nope");
        assert_eq!(unknown["severity"], "error");
    }

    #[test]
    fn a_findings_help_reaches_the_host_payload() {
        // `help` is the reason a finding is actionable, and this surface used
        // to be the one that dropped it: it built its own `Diagnostic` from
        // four fields of the `Finding` and threw the attachments away, so the
        // TypeScript plugin showed a bare "is not a defined function" where
        // the LSP and the CLI both said what to do about it.
        let source = "const q = db.query(\"RETURN fn::missing()\");\n";
        let value = run_json(&request("app.ts", source));
        let diagnostics = value["diagnostics"].as_array().expect("array");
        let unknown = diagnostics
            .iter()
            .find(|d| d["code"] == "E5001")
            .unwrap_or_else(|| panic!("expected E5001, got {diagnostics:?}"));
        let help = unknown["help"].as_array().expect("help present");
        assert_eq!(
            help[0]["message"],
            "no `DEFINE FUNCTION fn::missing` exists in the workspace"
        );
    }

    #[test]
    fn a_related_location_inside_the_file_lands_on_its_host_bytes() {
        // The definition and the bad call are both in this file, so the
        // related location is one the editor can actually jump to — and it
        // must be in host coordinates like every other offset we report.
        let source = "const q = db.query(`DEFINE FUNCTION fn::double($x: int) \
                      { RETURN $x * 2; }; RETURN fn::double(1, 2);`);\n";
        let value = run_json(&request("app.ts", source));
        let diagnostics = value["diagnostics"].as_array().expect("array");
        let arity = diagnostics
            .iter()
            .find(|d| d["code"] == "E5002")
            .unwrap_or_else(|| panic!("expected E5002, got {diagnostics:?}"));
        let related = arity["related"].as_array().expect("related present");
        let start = related[0]["start"].as_u64().expect("start") as usize;
        let end = related[0]["end"].as_u64().expect("end") as usize;
        assert_eq!(&source[start..end], "fn::double");
        assert!(start > source.find('`').expect("template opens"));
    }

    #[test]
    fn a_svelte_script_block_is_addressed_in_whole_file_coordinates() {
        let source = "<h1>hi</h1>\n<script lang=\"ts\">\n  \
                      const q = db.query(\"SELECT nope FROM account\");\n</script>\n";
        let value = run_json(&request("+page.svelte", source));
        let diagnostics = value["diagnostics"].as_array().expect("array");
        let unknown = diagnostics
            .iter()
            .find(|d| d["code"] == "E1002")
            .unwrap_or_else(|| panic!("expected E1002, got {diagnostics:?}"));
        let start = unknown["start"].as_u64().expect("start") as usize;
        let end = unknown["end"].as_u64().expect("end") as usize;
        assert_eq!(&source[start..end], "nope");
    }

    #[test]
    fn tokens_cover_the_query_and_stop_at_its_quotes() {
        let source = "const q = db.query(\"SELECT username FROM account\");\n";
        let value = run_json(&request("app.ts", source));
        let tokens = value["tokens"].as_array().expect("array");
        let covered: Vec<&str> = tokens
            .iter()
            .map(|token| {
                let start = token["start"].as_u64().expect("start") as usize;
                let end = token["end"].as_u64().expect("end") as usize;
                &source[start..end]
            })
            .collect();
        assert_eq!(covered, ["SELECT", "username", "FROM", "account"]);
        // Every token sits inside the string literal, never on the quote or
        // the call around it.
        let open = source.find('"').expect("quote") as u64;
        assert!(tokens
            .iter()
            .all(|token| token["start"].as_u64().expect("start") > open));
    }

    #[test]
    fn a_file_with_no_query_costs_nothing_and_says_nothing() {
        let value = run_json(&request("app.ts", "export const answer = 42;\n"));
        assert_eq!(value["diagnostics"].as_array().expect("array").len(), 0);
        assert_eq!(value["tokens"].as_array().expect("array").len(), 0);
        assert_eq!(value["queries"].as_array().expect("array").len(), 0);
    }

    #[test]
    fn an_empty_schema_says_every_table_is_unknown_and_that_is_the_callers_problem() {
        // Asked to analyze against no schema, the analyzer answers the way
        // `surrealql-analyzer check` on an empty workspace does: nothing is defined,
        // so nothing resolves. That is correct and it is also unusable in an
        // editor — which is exactly why "no config, no findings" is decided by
        // the caller (which knows whether the project opted in) and not here.
        // Pinning it means the plugin's silence is a deliberate gate rather
        // than an accident of what this function happens to return.
        let mut request = request("app.ts", "const q = db.query(\"SELECT name FROM person\");");
        request.schema = String::new();
        let value = run_json(&request);
        assert!(value["diagnostics"]
            .as_array()
            .expect("array")
            .iter()
            .any(|d| d["code"] == "E1001"));
        // Highlighting is a property of the query text alone, so it still
        // works without a schema.
        assert!(!value["tokens"].as_array().expect("array").is_empty());
    }

    #[test]
    fn hover_answers_at_a_host_offset_with_a_host_span() {
        let source = "const q = db.query(\"SELECT username FROM account\");\n";
        let mut request = request("app.ts", source);
        request.hover = Some(source.find("username").expect("field") as u32 + 2);
        let value = run_json(&request);
        let hover = &value["hover"];
        assert!(
            hover["markdown"]
                .as_str()
                .is_some_and(|md| md.contains("string")),
            "expected the declared type, got {hover}"
        );
        let start = hover["start"].as_u64().expect("start") as usize;
        let end = hover["end"].as_u64().expect("end") as usize;
        assert_eq!(&source[start..end], "username");
    }

    #[test]
    fn hover_outside_every_query_is_none() {
        let source = "const q = db.query(\"SELECT username FROM account\");\n";
        let mut request = request("app.ts", source);
        request.hover = Some(2);
        assert!(run_json(&request)["hover"].is_null());
    }

    #[test]
    fn a_substitution_is_a_parameter_not_an_unknown_name() {
        let source = "const n = 'ada';\nconst q = \
                      db.query(`SELECT username FROM account WHERE username = ${n}`);\n";
        let value = run_json(&request("app.ts", source));
        assert_eq!(
            value["diagnostics"].as_array().expect("array").len(),
            0,
            "interpolation must not manufacture a finding: {value}"
        );
    }

    #[test]
    fn a_write_query_takes_the_full_pass_and_still_reports_host_spans() {
        // `CREATE` can change the catalog, so it cannot use the incremental
        // path. The answer must be the same shape either way.
        let source = "await db.query(\"CREATE account SET nope = 1\");\n";
        let value = run_json(&request("app.ts", source));
        let diagnostics = value["diagnostics"].as_array().expect("array");
        let unknown = diagnostics
            .iter()
            .find(|d| d["code"] == "E1002")
            .unwrap_or_else(|| panic!("expected E1002, got {diagnostics:?}"));
        let start = unknown["start"].as_u64().expect("start") as usize;
        let end = unknown["end"].as_u64().expect("end") as usize;
        assert_eq!(&source[start..end], "nope");
    }

    #[test]
    fn the_catalog_is_rebuilt_when_the_schema_moves_and_not_before() {
        // The cache is keyed by schema text, so a changed schema must change
        // the answer on the very next call — a stale catalog would keep
        // flagging a field the user just added.
        let source = "const q = db.query(\"SELECT nickname FROM account\");";
        let unknown_fields = |value: &serde_json::Value| {
            value["diagnostics"]
                .as_array()
                .expect("array")
                .iter()
                .filter(|d| d["code"] == "E1002")
                .count()
        };
        assert_eq!(unknown_fields(&run_json(&request("app.ts", source))), 1);

        let mut widened = request("app.ts", source);
        widened
            .schema
            .push_str("DEFINE FIELD nickname ON account TYPE string;\n");
        let after = run_json(&widened);
        assert_eq!(
            unknown_fields(&after),
            0,
            "a schema edit must be visible immediately: {after}"
        );
    }

    #[test]
    fn the_hash_is_stable_across_calls() {
        // `DefaultHasher` is explicitly not stable; a key that moved for
        // identical input would rebuild the catalog on every keystroke and
        // quietly undo the entire caching strategy.
        assert_eq!(hash("DEFINE TABLE a;"), hash("DEFINE TABLE a;"));
        assert_ne!(hash("DEFINE TABLE a;"), hash("DEFINE TABLE b;"));
    }
}
