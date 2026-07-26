//! Editor-surface tests: drive the **real** `surrealguard-lsp` binary over
//! stdio and assert on what an editor would actually receive.
//!
//! Why a second layer on top of `backend.rs`: every regression a user reported
//! recently was invisible to unit tests that assert on internal APIs, because
//! the internal API was right and the editor surface was wrong. These tests
//! spawn the shipped binary, speak framed `Content-Length` JSON-RPC at it, and
//! check hover / inlay hints / completion / diagnostics at specific cursor
//! positions.
//!
//! The handshake is **sequenced** on purpose: `initialize`, wait for its
//! response, then `initialized`, then `didOpen`, then the request. Pipelining
//! these gets "Server not initialized" back from tower-lsp.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

/// The schema every editor-surface test opens first. Two relations with
/// **disjoint** IN/OUT sets, so a graph-slot completion offering the wrong one
/// cannot be excused as coincidence.
const SCHEMA: &str = "\
DEFINE TABLE account SCHEMAFULL;
DEFINE FIELD username ON account TYPE string;
DEFINE FIELD email ON account TYPE string;

DEFINE TABLE organization SCHEMAFULL;
DEFINE FIELD name ON organization TYPE string;
DEFINE FIELD tier ON organization TYPE 'free' | 'pro';
DEFINE FIELD note ON organization TYPE option<string | null>;
DEFINE FIELD owner ON organization TYPE record<account>;

DEFINE TABLE organization_unit SCHEMAFULL;
DEFINE FIELD headcount ON organization_unit TYPE int;

DEFINE TABLE organization_role SCHEMAFULL;
DEFINE FIELD title ON organization_role TYPE string;

DEFINE TABLE employee_of SCHEMAFULL TYPE RELATION FROM account TO organization;
DEFINE TABLE assigned_to SCHEMAFULL TYPE RELATION FROM organization_unit TO organization_role;
";

const SCHEMA_URI: &str = "file:///workspace/a_schema.surql";
const QUERY_URI: &str = "file:///workspace/b_query.surql";

/// A live `surrealguard-lsp` child process speaking LSP over its stdio.
struct Lsp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Drop for Lsp {
    fn drop(&mut self) {
        // Never leave a server behind, even when an assertion unwound the test.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Lsp {
    /// Spawns the binary and completes the handshake, in order.
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_surrealguard-lsp"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn surrealguard-lsp");

        let stdin = child.stdin.take().expect("child stdin");
        let stdout = BufReader::new(child.stdout.take().expect("child stdout"));
        let mut lsp = Lsp {
            child,
            stdin,
            stdout,
            next_id: 1,
        };

        // 1. initialize — and WAIT for the response before anything else.
        let result = lsp.request("initialize", json!({"capabilities": {}}));
        assert_eq!(
            result["serverInfo"]["name"], "surrealguard-lsp",
            "handshake must reach our server, got: {result}"
        );
        // 2. only now is the server allowed to accept notifications.
        lsp.notify("initialized", json!({}));
        lsp
    }

    fn send(&mut self, message: &Value) {
        let body = serde_json::to_string(message).expect("serialize message");
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len())
            .expect("write to server stdin");
        self.stdin.flush().expect("flush server stdin");
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// A request that carries no `params` at all — `shutdown`/`exit` reject a
    /// `null` params member.
    fn request_bare(&mut self, method: &str) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method}));
        loop {
            let message = self.read_message();
            if message.get("id").and_then(Value::as_i64) == Some(id) {
                assert!(
                    message.get("error").is_none(),
                    "{method} failed: {}",
                    message["error"]
                );
                return message.get("result").cloned().unwrap_or(Value::Null);
            }
        }
    }

    /// Sends a request and returns its `result`, draining the notifications
    /// (diagnostics, log messages) the server interleaves.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }));
        loop {
            let message = self.read_message();
            if message.get("id").and_then(Value::as_i64) == Some(id) {
                if let Some(error) = message.get("error") {
                    panic!("{method} failed: {error}");
                }
                return message.get("result").cloned().unwrap_or(Value::Null);
            }
        }
    }

    /// Opens a document and returns the diagnostics the server publishes for it.
    fn did_open(&mut self, uri: &str, text: &str) -> Vec<Value> {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": uri, "languageId": "surrealql", "version": 1, "text": text,
            }}),
        );
        self.next_publish(uri)
    }

    /// The next `publishDiagnostics` notification for `uri`.
    fn next_publish(&mut self, uri: &str) -> Vec<Value> {
        loop {
            let message = self.read_message();
            if message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri
            {
                return message["params"]["diagnostics"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
            }
        }
    }

    /// Reads one `Content-Length`-framed message.
    fn read_message(&mut self) -> Value {
        let mut length = None;
        loop {
            let mut line = String::new();
            let read = self
                .stdout
                .read_line(&mut line)
                .expect("read header from server");
            assert!(read != 0, "server closed its stdout before responding");
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                break;
            }
            if let Some(value) = trimmed.strip_prefix("Content-Length:") {
                length = Some(value.trim().parse::<usize>().expect("numeric length"));
            }
        }
        let length = length.expect("framed message carries Content-Length");
        let mut body = vec![0u8; length];
        self.stdout
            .read_exact(&mut body)
            .expect("read body from server");
        serde_json::from_slice(&body).expect("server sent valid JSON-RPC")
    }

    /// Opens the shared schema, then `query`, and returns the query's
    /// diagnostics. Both documents are open, which is how the editor resolves
    /// a query against a schema living in another file.
    fn with_schema(query: &str) -> (Self, Vec<Value>) {
        let mut lsp = Lsp::start();
        let _ = lsp.did_open(SCHEMA_URI, SCHEMA);
        let diagnostics = lsp.did_open(QUERY_URI, query);
        (lsp, diagnostics)
    }

    fn completion_labels(&mut self, text: &str, cursor: usize) -> Vec<String> {
        let position = position_of(text, cursor);
        let result = self.request(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": QUERY_URI},
                "position": position,
                "context": {"triggerKind": 1},
            }),
        );
        let items = result
            .as_array()
            .cloned()
            .or_else(|| result["items"].as_array().cloned())
            .unwrap_or_default();
        items
            .iter()
            .map(|item| item["label"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    fn hover_markdown(&mut self, text: &str, cursor: usize) -> Option<String> {
        let position = position_of(text, cursor);
        let result = self.request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": QUERY_URI},
                "position": position,
            }),
        );
        result["contents"]["value"]
            .as_str()
            .map(std::string::ToString::to_string)
    }
}

/// UTF-8 byte offset → LSP `{line, character}`. The corpus in these tests is
/// ASCII, so a byte offset is a character offset.
fn position_of(text: &str, offset: usize) -> Value {
    let before = &text[..offset];
    let line = before.matches('\n').count();
    let character = before.len() - before.rfind('\n').map_or(0, |index| index + 1);
    json!({"line": line, "character": character})
}

/// The byte offset just past the `n`th occurrence of `needle`.
fn after(text: &str, needle: &str, n: usize) -> usize {
    let mut from = 0;
    for _ in 0..n {
        let found = text[from..]
            .find(needle)
            .unwrap_or_else(|| panic!("`{needle}` occurrence {n} not found"));
        from += found + needle.len();
    }
    from
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

#[test]
fn the_real_server_publishes_diagnostics_for_a_known_bad_query() {
    let (_lsp, diagnostics) = Lsp::with_schema("SELECT * FROM persn;\n");

    let unknown_table = diagnostics
        .iter()
        .find(|d| d["code"] == "E1001")
        .unwrap_or_else(|| panic!("expected E1001 for the misspelled table, got: {diagnostics:?}"));
    // Severity 1 == ERROR.
    assert_eq!(unknown_table["severity"], 1);
    assert_eq!(unknown_table["range"]["start"]["line"], 0);
    assert_eq!(unknown_table["range"]["start"]["character"], 14);
    assert_eq!(unknown_table["range"]["end"]["character"], 19);
}

#[test]
fn the_real_server_reports_a_clean_query_clean() {
    let (_lsp, diagnostics) = Lsp::with_schema("SELECT name, tier FROM organization;\n");
    let errors: Vec<&Value> = diagnostics.iter().filter(|d| d["severity"] == 1).collect();
    assert!(
        errors.is_empty(),
        "valid input must produce no editor errors: {errors:?}"
    );
}

// ---------------------------------------------------------------------------
// Completion
// ---------------------------------------------------------------------------

#[test]
fn completion_in_a_graph_slot_offers_only_edges_the_receiver_can_traverse() {
    // `account` is the IN side of `employee_of` and touches `assigned_to`
    // nowhere, so `assigned_to` must not be offered here. This is the exact
    // shape of the reported bug: a graph slot listing edges the receiver
    // cannot traverse.
    let query = "SELECT ->\nFROM account;\n";
    let (mut lsp, _) = Lsp::with_schema(query);
    let labels = lsp.completion_labels(query, after(query, "->", 1));

    assert!(
        labels.iter().any(|label| label == "employee_of"),
        "the traversable edge must be offered, got: {labels:?}"
    );
    assert!(
        !labels.iter().any(|label| label == "assigned_to"),
        "`assigned_to` goes organization_unit -> organization_role and is NOT \
         traversable from `account`; offering it is the regression this test \
         exists for. Got: {labels:?}"
    );
}

#[test]
fn completion_after_a_dot_follows_a_record_link_into_the_target_table() {
    // `organization.owner` is a `record<account>`, so the members after the dot
    // must come from `account` — not from `organization`, and not from nothing.
    let query = "SELECT owner. FROM organization;\n";
    let (mut lsp, _) = Lsp::with_schema(query);
    let labels = lsp.completion_labels(query, after(query, "owner.", 1));

    for expected in ["username", "email"] {
        assert!(
            labels.iter().any(|label| label == expected),
            "`{expected}` lives on the linked `account` and must be offered \
             after `owner.`, got: {labels:?}"
        );
    }
    assert!(
        !labels.iter().any(|label| label == "tier"),
        "`tier` is a field of `organization`, not of the linked `account`; \
         offering it means the link was not followed. Got: {labels:?}"
    );
}

#[test]
fn completion_offers_the_row_table_fields_in_a_projection_slot() {
    let query = "SELECT  FROM organization;\n";
    let (mut lsp, _) = Lsp::with_schema(query);
    let labels = lsp.completion_labels(query, after(query, "SELECT ", 1));

    for expected in ["name", "tier", "owner"] {
        assert!(
            labels.iter().any(|label| label == expected),
            "`{expected}` is a field of the row table and must be offered, got: {labels:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Hover
// ---------------------------------------------------------------------------

/// The narrowing-guard hover (NEW-11). `$org` is `option<…>` where it is bound,
/// and after the `IF $org = NONE THEN THROW` guard it is provably non-`NONE`.
/// Hover must answer **per occurrence**: the declared kind at and before the
/// guard, the narrowed kind after it — never a single kind for the whole file,
/// which is what made the editor contradict the analyzer.
#[test]
fn hover_reports_the_declared_kind_at_a_guard_and_the_narrowed_kind_after_it() {
    let query = "\
LET $org = (SELECT name, tier FROM ONLY organization LIMIT 1);
IF $org = NONE THEN THROW 'missing' END;
RETURN $org.name;
";
    let (mut lsp, _) = Lsp::with_schema(query);

    let at_binding = lsp
        .hover_markdown(query, after(query, "$org", 1) - 1)
        .expect("hover on the binding site");
    let at_guard = lsp
        .hover_markdown(query, after(query, "$org", 2) - 1)
        .expect("hover on the guard");
    let after_guard = lsp
        .hover_markdown(query, after(query, "$org", 3) - 1)
        .expect("hover after the guard");

    // The binding site mirrors the binding as written, so it keeps the
    // author's `option<…>` spelling.
    assert!(
        at_binding.contains("option<"),
        "`FROM ONLY … LIMIT 1` yields an option; hover said: {at_binding}"
    );
    // An occurrence answers a different question — what can this be *here* —
    // so it names the members that are live rather than folding them into a
    // wrapper that says the declaration was optional. At the guard the `none`
    // is one of them, or the test would be pointless.
    assert!(
        at_guard.contains("none"),
        "at the guard the binding can still be NONE; hover said: {at_guard}"
    );

    // Past the diverging guard the binding cannot be NONE, and hover says so.
    assert!(
        !after_guard.contains("none"),
        "the guard removed NONE, so hover past it must not report it as live; \
         hover said: {after_guard}"
    );
    // The narrowing takes away the option marker and nothing else.
    assert!(
        after_guard.contains("name") && after_guard.contains("tier"),
        "narrowing must keep the row shape, hover said: {after_guard}"
    );
    assert_ne!(
        at_binding, after_guard,
        "the declared and narrowed kinds must differ — an equal pair means \
         hover went back to reporting one kind per binding (NEW-11)"
    );
}

/// The other half of "only beyond the reduction site": a guard whose body does
/// not divert proves nothing past the `IF`, so the narrowing must stop at the
/// body's end and the statements after it hover as declared again.
#[test]
fn hover_narrows_inside_a_guard_body_but_not_after_the_if() {
    let query = "\
LET $org = (SELECT name, tier FROM ONLY organization LIMIT 1);
IF $org != NONE THEN UPDATE organization SET name = $org.name END;
RETURN $org;
";
    let (mut lsp, _) = Lsp::with_schema(query);

    let in_body = lsp
        .hover_markdown(query, after(query, "$org", 3) - 1)
        .expect("hover inside the THEN body");
    let after_if = lsp
        .hover_markdown(query, after(query, "$org", 4) - 1)
        .expect("hover after the IF");

    assert!(
        !in_body.contains("none"),
        "inside the body the condition holds, so the binding cannot be NONE; \
         hover said: {in_body}"
    );
    assert!(
        after_if.contains("none"),
        "the IF has no ELSE and does not divert, so nothing is proven after it; \
         hover said: {after_if}"
    );
}

/// A narrowed occurrence reports **which members survived**, not that the
/// declaration was optional. `note` is `option<string | null>`; past a
/// `= NULL` guard that throws, a `null` can no longer reach the read — but a
/// `none` still can, because `NULL = NONE` is FALSE on the engine. The one
/// spelling that says all of that is `none | string`. `option<string>` — the
/// declared spelling of the same kind — says "this was declared optional",
/// which is a different sentence and is not the one the reader needs here.
#[test]
fn hover_at_a_narrowed_occurrence_names_the_surviving_members() {
    let query = "\
LET $note = (SELECT VALUE note FROM ONLY organization LIMIT 1);
IF $note = NULL THEN THROW 'null note' END;
RETURN $note;
";
    let (mut lsp, _) = Lsp::with_schema(query);

    let at_binding = lsp
        .hover_markdown(query, after(query, "$note", 1) - 1)
        .expect("hover on the binding site");
    let after_guard = lsp
        .hover_markdown(query, after(query, "$note", 3) - 1)
        .expect("hover after the guard");

    assert!(
        at_binding.contains("option<string | null>"),
        "the definition site mirrors the declaration; hover said: {at_binding}"
    );
    assert!(
        after_guard.contains("none | string"),
        "the guard removed the `null` and left the `none`; hover said: {after_guard}"
    );
}

#[test]
fn hover_on_a_schema_field_reports_its_declared_type() {
    let query = "SELECT tier FROM organization;\n";
    let (mut lsp, _) = Lsp::with_schema(query);
    let markdown = lsp
        .hover_markdown(query, after(query, "tie", 1))
        .expect("hover on a projected field");
    assert!(
        markdown.contains("'free'") && markdown.contains("'pro'"),
        "the literal union must survive to the editor, hover said: {markdown}"
    );
}

// ---------------------------------------------------------------------------
// Inlay hints
// ---------------------------------------------------------------------------

#[test]
fn inlay_hints_annotate_a_let_binding_with_its_inferred_type() {
    let query = "\
LET $orgs = (SELECT name FROM organization);
LET $count = array::len($orgs);
RETURN $count;
";
    let (mut lsp, _) = Lsp::with_schema(query);
    let result = lsp.request(
        "textDocument/inlayHint",
        json!({
            "textDocument": {"uri": QUERY_URI},
            "range": {
                "start": {"line": 0, "character": 0},
                "end": {"line": 3, "character": 0},
            },
        }),
    );
    let hints = result.as_array().cloned().unwrap_or_default();
    let labels: Vec<String> = hints
        .iter()
        .map(|hint| hint["label"].as_str().unwrap_or_default().to_string())
        .collect();

    assert!(
        labels.iter().any(|label| label.contains("array<")),
        "`$orgs` binds a row array and must be annotated as one, got: {labels:?}"
    );
    assert!(
        labels.iter().any(|label| label.contains("int")),
        "`array::len` returns an int and the hint must say so, got: {labels:?}"
    );
    // A hint has to sit at the end of the `$name` token or it renders in the
    // wrong place — a class of bug an internal-API test cannot see.
    let orgs_hint = hints
        .iter()
        .find(|hint| {
            hint["label"]
                .as_str()
                .unwrap_or_default()
                .contains("array<")
        })
        .expect("hint present");
    assert_eq!(orgs_hint["position"]["line"], 0);
    assert_eq!(orgs_hint["position"]["character"], 9);
}

// ---------------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------------

#[test]
fn the_binary_advertises_the_editor_surfaces_these_tests_exercise() {
    let mut lsp = Lsp::start();
    let result = lsp.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": "file:///workspace/never_opened.surql"},
            "position": {"line": 0, "character": 0},
        }),
    );
    // An unopened document must answer, not hang or error.
    assert!(result.is_null() || result.is_array() || result.is_object());

    // And the server shuts down cleanly when asked.
    let _ = lsp.request_bare("shutdown");
    lsp.send(&json!({"jsonrpc": "2.0", "method": "exit"}));
    std::thread::sleep(Duration::from_millis(100));
}
