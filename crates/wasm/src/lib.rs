//! WebAssembly bindings for the SurrealGuard analyzer.
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

use std::alloc::{alloc as global_alloc, dealloc as global_dealloc, Layout};

use serde::Serialize;
use surrealguard_diagnostics::Severity;
use surrealguard_workspace::analysis::{analyze_query, Workspace};
use surrealguard_workspace::{render, KindContext};

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

    serde_json::to_string(&diagnostics).unwrap_or_else(|_| "[]".to_string())
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
}
