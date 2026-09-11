//! WebAssembly bindings for the SurrealQL Analyzer engine.
//!
//! The crate compiles to `wasm32-wasip1` and exposes a tiny C ABI so a
//! browser (or a Node/WASI harness) can call
//! `analyze(schemaText, queryText)` and receive a JSON array of
//! diagnostics. No `wasm-bindgen` is involved — the ABI is three exported
//! functions plus linear-memory string passing, which any WASI shim can
//! drive.
//!
//! ## ABI
//!
//! - `sg_alloc(len) -> ptr` — reserve `len` bytes of guest memory so the
//!   host can write a UTF-8 string into it.
//! - `sg_dealloc(ptr, len)` — release a buffer previously handed out by
//!   `sg_alloc` or returned by `sg_analyze`.
//! - `sg_analyze(schema_ptr, schema_len, query_ptr, query_len) -> u64` —
//!   run the analyzer. The result packs the pointer to a freshly allocated
//!   UTF-8 JSON buffer in the high 32 bits and its length in the low 32
//!   bits. The host reads `memory[ptr..ptr+len]`, then frees it with
//!   `sg_dealloc`.
//! - `sg_analyze2(...) -> u64` — same call, richer payload: an object
//!   `{ diagnostics, statements }` where `statements` carries the inferred
//!   response kind of each top-level query statement.
//!
//! `sg_analyze`'s JSON payload is an array of
//! `{ code, severity, message, start, end }` objects, one per diagnostic
//! raised on the query source.
//!
//! ## Why `sg_analyze2` is a second export rather than a wider `sg_analyze`
//!
//! The `.wasm` is a plain file on a CDN and the `.mjs` that drives it is
//! another; a browser can hold a fresh script against a cached module or the
//! reverse. Changing `sg_analyze`'s payload shape would make those two
//! combinations fail — silently, in the direction that matters (an old
//! bundle, a new page). A second export is feature-detectable
//! (`typeof exports.sg_analyze2 === "function"`), so a page that wants type
//! chips asks for them and does without when the module predates them, while
//! diagnostics — the thing the playground exists for — keep working either
//! way.

pub mod host;

use std::alloc::{alloc as global_alloc, dealloc as global_dealloc, Layout};

use serde::Serialize;
use surrealql_analyzer_diagnostics::Severity;
use surrealql_analyzer_workspace::analysis::{analyze_query, Workspace};
use surrealql_analyzer_workspace::{render, KindContext};

/// One diagnostic in the shape the playground consumes.
#[derive(Serialize)]
struct Diagnostic {
    /// Stable finding code, e.g. `"E1002"`.
    code: String,
    /// `"error"`, `"warning"`, or `"hint"`.
    severity: &'static str,
    /// Human-readable message.
    message: String,
    /// Byte offset of the diagnostic span start, in the query source.
    start: u32,
    /// Byte offset of the diagnostic span end, in the query source.
    end: u32,
}

/// One top-level query statement, with the type the engine says it
/// responds with.
#[derive(Serialize)]
struct Statement {
    /// Stable statement-kind name (`"select"`, `"relate"`, ...).
    kind: String,
    /// Byte offset of the statement start, in the query source.
    start: u32,
    /// Byte offset of the statement end, in the query source.
    end: u32,
    /// The rendered response kind, or `null` when the statement does not
    /// respond with a value (a `DEFINE`, say) or the engine could not
    /// determine one.
    response: Option<String>,
}

/// Both halves of a run, in query-relative coordinates.
#[derive(Serialize)]
struct Analysis {
    diagnostics: Vec<Diagnostic>,
    statements: Vec<Statement>,
}

fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Hint => "hint",
    }
}

/// Analyzes `query` against `schema` and returns the query's diagnostics
/// as a JSON array string.
///
/// The schema and query are concatenated into a single source and walked
/// together, so the shared `SchemaIndex` is populated by the schema
/// statements before the query statements are checked (analysis
/// accumulates schema effects in statement order within one source).
/// Diagnostics falling inside the schema prefix are dropped; those in the
/// query are remapped to query-relative byte offsets.
pub fn analyze(schema: &str, query: &str) -> String {
    serde_json::to_string(&run(schema, query).diagnostics).unwrap_or_else(|_| "[]".to_string())
}

/// Analyzes `query` against `schema` and returns
/// `{ diagnostics, statements }` as a JSON object string.
///
/// Same run as [`analyze`], same schema-prefix clipping — the extra half is
/// the inferred response kind of each top-level statement, which is what the
/// playground draws as a type chip.
pub fn analyze_with_types(schema: &str, query: &str) -> String {
    serde_json::to_string(&run(schema, query))
        .unwrap_or_else(|_| r#"{"diagnostics":[],"statements":[]}"#.to_string())
}

/// The one analysis both exports are views of.
///
/// Response kinds are rendered with [`KindContext::Occurrence`], which spells
/// optionality out (`none | string`) instead of folding it into
/// `option<string>`. A response kind is not something the author wrote — there
/// is no `DEFINE` for the shape a `SELECT` hands back — so `Declared`, whose
/// job is to mirror a spelling the reader can go and look at, has nothing to
/// mirror here. What the reader of a result actually needs to know is which
/// members they must handle, and a folded `option<…>` reads as "the schema
/// said optional" rather than "this value may be absent". It is also the
/// context hover uses for a value's type *at a position*, so a chip and an
/// editor popover never disagree about the same kind.
fn run(schema: &str, query: &str) -> Analysis {
    // A newline separates the last schema statement from the query and keeps
    // byte offsets easy to remap.
    let prefix_len = schema.len() as u32 + 1;
    let combined = format!("{schema}\n{query}");

    let mut workspace = Workspace::default();
    let output = analyze_query(&mut workspace, &combined);

    let diagnostics: Vec<Diagnostic> = output
        .diagnostics
        .iter()
        .filter_map(|finding| {
            let start = finding.span().range().start();
            let end = finding.span().range().end();
            // Keep only diagnostics that land in the query region.
            if start < prefix_len {
                return None;
            }
            Some(Diagnostic {
                code: finding.code().to_string(),
                severity: severity_label(finding.severity()),
                message: finding.message().to_string(),
                start: start - prefix_len,
                end: end.saturating_sub(prefix_len),
            })
        })
        .collect();

    let statements: Vec<Statement> = output
        .statements
        .iter()
        .filter_map(|statement| {
            let start = statement.span.range().start();
            let end = statement.span.range().end();
            // The schema statements were analyzed too; they are not the
            // user's query and have no chip.
            if start < prefix_len {
                return None;
            }
            Some(Statement {
                kind: statement.kind.clone(),
                start: start - prefix_len,
                end: end.saturating_sub(prefix_len),
                response: statement
                    .response_kind
                    .as_ref()
                    .map(|kind| render(kind, KindContext::Occurrence { proved: None }).text),
            })
        })
        .collect();

    Analysis {
        diagnostics,
        statements,
    }
}

// ---------------------------------------------------------------------------
// C ABI surface
// ---------------------------------------------------------------------------

/// Allocate `len` bytes of guest memory for the host to write into.
///
/// # Safety
/// The returned pointer must eventually be released with [`sg_dealloc`]
/// using the same `len`.
#[no_mangle]
pub extern "C" fn sg_alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::null_mut();
    }
    let layout = Layout::from_size_align(len, 1).expect("valid layout");
    // SAFETY: len is non-zero, layout is valid.
    unsafe { global_alloc(layout) }
}

/// Release a buffer handed out by [`sg_alloc`] or returned by
/// [`sg_analyze`].
///
/// # Safety
/// `ptr`/`len` must match a live allocation from this module.
#[no_mangle]
pub unsafe extern "C" fn sg_dealloc(ptr: *mut u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    let layout = Layout::from_size_align(len, 1).expect("valid layout");
    global_dealloc(ptr, layout);
}

/// Run the analyzer over the schema/query strings in guest memory.
///
/// Returns a `u64` packing `(result_ptr << 32) | result_len`, where the
/// result buffer holds UTF-8 JSON. The caller owns the result buffer and
/// must free it with [`sg_dealloc`].
///
/// # Safety
/// Both `(schema_ptr, schema_len)` and `(query_ptr, query_len)` must
/// describe valid UTF-8 buffers in guest memory.
#[no_mangle]
pub unsafe extern "C" fn sg_analyze(
    schema_ptr: *const u8,
    schema_len: usize,
    query_ptr: *const u8,
    query_len: usize,
) -> u64 {
    let schema = str_from_raw(schema_ptr, schema_len);
    let query = str_from_raw(query_ptr, query_len);

    let json = analyze(&schema, &query);

    let bytes = json.into_bytes().into_boxed_slice();
    let len = bytes.len();
    let ptr = Box::into_raw(bytes) as *mut u8;
    ((ptr as u64) << 32) | (len as u64)
}

/// Run the analyzer and return diagnostics *and* per-statement response
/// kinds.
///
/// Result packing and ownership are identical to [`sg_analyze`]; only the
/// JSON shape differs — `{ diagnostics: [...], statements: [...] }`.
///
/// # Safety
/// Both `(schema_ptr, schema_len)` and `(query_ptr, query_len)` must
/// describe valid UTF-8 buffers in guest memory.
#[no_mangle]
pub unsafe extern "C" fn sg_analyze2(
    schema_ptr: *const u8,
    schema_len: usize,
    query_ptr: *const u8,
    query_len: usize,
) -> u64 {
    let schema = str_from_raw(schema_ptr, schema_len);
    let query = str_from_raw(query_ptr, query_len);

    let json = analyze_with_types(&schema, &query);

    let bytes = json.into_bytes().into_boxed_slice();
    let len = bytes.len();
    let ptr = Box::into_raw(bytes) as *mut u8;
    ((ptr as u64) << 32) | (len as u64)
}

/// Analyze a whole **host file** — a `.ts`, `.tsx`, `.svelte`, … — and answer
/// diagnostics, highlighting tokens, query extents and (optionally) a hover in
/// one call, all at host-file byte offsets.
///
/// The argument is a single JSON object rather than a widening list of string
/// pairs, because this is the export an editor plugin calls on a keystroke and
/// the set of questions it asks will grow. A JSON request costs one parse of a
/// few hundred bytes against an analysis measured in milliseconds, and it
/// means adding a question never renumbers an argument.
///
/// See [`host::Request`] / [`host::Response`] for the shapes. Result packing
/// and ownership are identical to [`sg_analyze`].
///
/// # Safety
/// `(request_ptr, request_len)` must describe a valid UTF-8 buffer in guest
/// memory.
#[no_mangle]
pub unsafe extern "C" fn sg_host(request_ptr: *const u8, request_len: usize) -> u64 {
    let request = str_from_raw(request_ptr, request_len);

    let json = match serde_json::from_str::<host::Request>(&request) {
        Ok(request) => host::analyze_host(&request),
        // A request we cannot read is a bug in the caller, not something to
        // paint in someone's editor: answer "nothing to say".
        Err(_) => r#"{"diagnostics":[],"tokens":[],"queries":[],"hover":null}"#.to_string(),
    };

    let bytes = json.into_bytes().into_boxed_slice();
    let len = bytes.len();
    let ptr = Box::into_raw(bytes) as *mut u8;
    ((ptr as u64) << 32) | (len as u64)
}

/// Reconstruct an owned `String` from a guest-memory buffer. Copies so the
/// caller's buffer lifetime is irrelevant. Invalid UTF-8 is replaced.
unsafe fn str_from_raw(ptr: *const u8, len: usize) -> String {
    if ptr.is_null() || len == 0 {
        return String::new();
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    String::from_utf8_lossy(slice).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_field_is_flagged() {
        let json = analyze(
            "DEFINE TABLE user SCHEMAFULL; DEFINE FIELD name ON user TYPE string;",
            "SELECT ssn FROM user",
        );
        assert!(json.contains("E1002"), "expected E1002 in {json}");
    }

    #[test]
    fn statements_carry_query_relative_spans_and_a_response_kind() {
        let schema = "DEFINE TABLE user SCHEMAFULL; DEFINE FIELD name ON user TYPE string;";
        let query = "SELECT name FROM user;";
        let json = analyze_with_types(schema, query);
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid json");

        let statements = value["statements"].as_array().expect("statements array");
        assert_eq!(
            statements.len(),
            1,
            "schema statements must be clipped: {json}"
        );
        assert_eq!(statements[0]["kind"], "select");
        // Query-relative: the SELECT starts at byte 0 of the query pane.
        assert_eq!(statements[0]["start"], 0);
        assert_eq!(
            statements[0]["response"], "array<{ name: string }>",
            "unexpected response kind in {json}"
        );
    }

    #[test]
    fn a_non_responding_statement_has_a_null_response() {
        let json = analyze_with_types("", "DEFINE TABLE thing SCHEMAFULL;");
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        let statements = value["statements"].as_array().expect("statements array");
        assert_eq!(statements.len(), 1);
        assert!(statements[0]["response"].is_null(), "{json}");
    }

    #[test]
    fn optionality_is_spelled_out_not_folded() {
        // `KindContext::Occurrence`: the reader of a result must handle the
        // `none`, so it stays visible rather than folding into `option<…>`.
        let json = analyze_with_types(
            "DEFINE TABLE user SCHEMAFULL; DEFINE FIELD nick ON user TYPE option<string>;",
            "SELECT nick FROM user;",
        );
        assert!(
            json.contains("none | string"),
            "expected a spelled-out union in {json}"
        );
    }

    #[test]
    fn the_legacy_export_still_returns_a_bare_array() {
        // Feature detection only helps if the old shape is genuinely
        // untouched: a page holding a new module must keep working.
        let json = analyze("", "SELECT 1;");
        assert!(
            json.starts_with('['),
            "sg_analyze must stay an array: {json}"
        );
    }
}
