//! Editor-surface tests: drive the **real** `surrealql-analyzer-lsp` binary over
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
//!
//! The server also sends **requests** of its own (`workspace/semanticTokens/
//! refresh`), and tower-lsp numbers those from 0 — the same ids this harness
//! uses for its requests. A message is a response only when it carries an
//! `id` and **no** `method`; matching on `id` alone once took the server's
//! refresh request for the hover response and failed twelve of these tests
//! at once. Like a real editor, the harness answers every server request (a
//! `null` result), so the server is exercised with the protocol it will
//! actually get — unless a test opts out to observe what an unanswering
//! client provokes.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
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

/// A live `surrealql-analyzer-lsp` child process speaking LSP over its stdio.
struct Lsp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
    /// The `initialize` result, kept so a test can assert on what the server
    /// advertised without a second (illegal) handshake.
    capabilities: Value,
    /// The method of every server→client request seen so far, in order.
    server_requests: Vec<String>,
    /// Whether to answer server→client requests (a real editor does). A test
    /// turns this off to observe what a client that never answers provokes.
    answer_server_requests: bool,
}

impl Drop for Lsp {
    fn drop(&mut self) {
        // Never leave a server behind, even when an assertion unwound the test.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Lsp {
    /// Spawns the binary and completes the handshake, in order, as a client
    /// advertising no capabilities that answers every server request.
    fn start() -> Self {
        Self::start_as(json!({}), true)
    }

    /// Spawns the binary and completes the handshake as a client with the
    /// given `ClientCapabilities`, answering server requests or not.
    fn start_as(client_capabilities: Value, answer_server_requests: bool) -> Self {
        Self::handshake(
            json!({"capabilities": client_capabilities}),
            answer_server_requests,
        )
    }

    /// The handshake a *modern* editor performs: a workspace root on disk, and
    /// the two capabilities the suppression actions are gated behind — code
    /// action literals, and file watching so an edited `surrealql-analyzer.toml` is
    /// noticed. Server requests are answered, as a real editor would.
    fn start_in(root: &Path) -> Self {
        Self::handshake(
            json!({
                "capabilities": {
                    "textDocument": {
                        "codeAction": {
                            "codeActionLiteralSupport": {
                                "codeActionKind": {"valueSet": ["quickfix"]},
                            },
                        },
                    },
                    "workspace": {
                        "didChangeWatchedFiles": {"dynamicRegistration": true},
                    },
                },
                "workspaceFolders": [{"uri": file_uri(root), "name": "workspace"}],
            }),
            true,
        )
    }

    /// Spawns the binary and runs the sequenced handshake with the given
    /// `initialize` params.
    fn handshake(params: Value, answer_server_requests: bool) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_surrealql-analyzer-lsp"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn surrealql-analyzer-lsp");

        let stdin = child.stdin.take().expect("child stdin");
        let stdout = BufReader::new(child.stdout.take().expect("child stdout"));
        let mut lsp = Lsp {
            child,
            stdin,
            stdout,
            next_id: 1,
            capabilities: Value::Null,
            server_requests: Vec::new(),
            answer_server_requests,
        };

        // 1. initialize — and WAIT for the response before anything else.
        let result = lsp.request("initialize", params);
        assert_eq!(
            result["serverInfo"]["name"], "surrealql-analyzer-lsp",
            "handshake must reach our server, got: {result}"
        );
        lsp.capabilities = result["capabilities"].clone();
        // 2. only now is the server allowed to accept notifications.
        lsp.notify("initialized", json!({}));
        lsp
    }

    /// The semantic-token legend the server advertised at handshake.
    fn token_legend(&self) -> Vec<Value> {
        self.capabilities["semanticTokensProvider"]["legend"]["tokenTypes"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "the server must advertise a legend, got: {}",
                    self.capabilities
                )
            })
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
            if Self::is_response_to(&message, id) {
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
            if Self::is_response_to(&message, id) {
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

    /// Whether `message` is the response to our request `id` — and not a
    /// server→client request that happens to carry the same number.
    fn is_response_to(message: &Value, id: i64) -> bool {
        message.get("id").and_then(Value::as_i64) == Some(id) && message.get("method").is_none()
    }

    /// How many `workspace/semanticTokens/refresh` requests the server has
    /// sent so far.
    fn refresh_requests(&self) -> usize {
        self.server_requests
            .iter()
            .filter(|method| *method == "workspace/semanticTokens/refresh")
            .count()
    }

    /// Reads one `Content-Length`-framed message. A server→client request is
    /// recorded (and answered, when the harness plays a well-behaved client)
    /// and then returned like any other message, so callers can never mistake
    /// it for the response they wait for.
    fn read_message(&mut self) -> Value {
        let message = self.read_framed();
        if let (Some(id), Some(method)) = (
            message.get("id"),
            message.get("method").and_then(Value::as_str),
        ) {
            self.server_requests.push(method.to_string());
            if self.answer_server_requests {
                self.send(&json!({"jsonrpc": "2.0", "id": id, "result": Value::Null}));
            }
        }
        message
    }

    /// Gives a spawned server task time to write, then drains everything the
    /// server has written by round-tripping one request: the pipe is FIFO, so
    /// by the time the response arrives every earlier message has been read.
    fn settle(&mut self) {
        std::thread::sleep(Duration::from_millis(200));
        let _ = self.request(
            "textDocument/hover",
            json!({"textDocument": {"uri": SCHEMA_URI}, "position": {"line": 0, "character": 0}}),
        );
    }

    /// Replaces a document's text and returns the diagnostics published for it.
    fn did_change(&mut self, uri: &str, version: i64, text: &str) -> Vec<Value> {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": text}],
            }),
        );
        self.next_publish(uri)
    }

    /// Reads one raw `Content-Length`-framed message.
    fn read_framed(&mut self) -> Value {
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

    /// The code actions offered for one published diagnostic, asked for
    /// exactly the way an editor asks: the diagnostic's own range, and the
    /// diagnostic itself in the request context.
    fn code_actions(&mut self, uri: &str, diagnostic: &Value) -> Vec<Value> {
        let result = self.request(
            "textDocument/codeAction",
            json!({
                "textDocument": {"uri": uri},
                "range": diagnostic["range"],
                "context": {"diagnostics": [diagnostic], "triggerKind": 1},
            }),
        );
        result.as_array().cloned().unwrap_or_default()
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

/// A workspace root on disk, removed when the test that made it finishes.
struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new(tag: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "surrealql-analyzer-code-action-{tag}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create workspace root");
        TempRoot { path }
    }

    /// Writes a file into the root and returns its `file://` URI.
    fn write(&self, name: &str, text: &str) -> String {
        let path = self.path.join(name);
        std::fs::write(&path, text).expect("write workspace file");
        file_uri(&path)
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.path.join(name)).expect("read workspace file")
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// The `file://` URI form the server itself produces for a path, so a document
/// the server scanned off disk and one the test opens are the same document.
fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_str().expect("utf-8 path").bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                uri.push(byte as char);
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

/// Applies a `TextEdit`'s JSON to text, the way the client would.
fn apply_lsp_edit(text: &str, edit: &Value) -> String {
    let start = offset_at(text, &edit["range"]["start"]);
    let end = offset_at(text, &edit["range"]["end"]);
    let mut result = text.to_string();
    result.replace_range(
        start..end,
        edit["newText"].as_str().expect("edit carries newText"),
    );
    result
}

/// `{line, character}` → byte offset. The fixtures here are ASCII.
fn offset_at(text: &str, position: &Value) -> usize {
    let line = position["line"].as_u64().expect("line") as usize;
    let character = position["character"].as_u64().expect("character") as usize;
    let mut offset = 0;
    for _ in 0..line {
        offset += text[offset..].find('\n').expect("line exists") + 1;
    }
    offset + character
}

/// The single-file edit an action carries, as `(uri, edit)`.
fn sole_edit(action: &Value) -> (String, Value) {
    let changes = action["edit"]["changes"]
        .as_object()
        .unwrap_or_else(|| panic!("action must carry a `changes` workspace edit: {action}"));
    assert_eq!(changes.len(), 1, "one file per action: {action}");
    let (uri, edits) = changes.iter().next().expect("one entry");
    let edits = edits.as_array().expect("edit list");
    assert_eq!(edits.len(), 1, "one edit per action: {action}");
    (uri.clone(), edits[0].clone())
}

fn titles(actions: &[Value]) -> Vec<String> {
    actions
        .iter()
        .map(|action| action["title"].as_str().unwrap_or_default().to_string())
        .collect()
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

/// Parentheses are a semantic no-op in SurrealQL, so `IF ($org = NONE)` proves
/// exactly what `IF $org = NONE` proves and the editor must say the same thing
/// at the same occurrence. This is the surface half of the equivalence the
/// expression-fact layer is built on: a guard is recognized by what it
/// *denotes*, and a pair of parentheses changes nothing about that.
///
/// It is pinned at the editor surface because that is where it was observed
/// broken — a parenthesized guard used to leave hover reporting the declared
/// kind past a guard that had already ruled the `none` out.
#[test]
fn hover_past_a_parenthesized_guard_matches_the_unparenthesized_one() {
    let plain = "\
LET $org = (SELECT name, tier FROM ONLY organization LIMIT 1);
IF $org = NONE THEN THROW 'no organization' END;
RETURN $org;
";
    let parenthesized = "\
LET $org = (SELECT name, tier FROM ONLY organization LIMIT 1);
IF ($org = NONE) THEN THROW 'no organization' END;
RETURN $org;
";
    // The third `$org` is the read past the diverging guard in both spellings.
    let past_guard = |query: &str| {
        let (mut lsp, _) = Lsp::with_schema(query);
        lsp.hover_markdown(query, after(query, "$org", 3) - 1)
            .expect("hover past the guard")
    };

    let plain_hover = past_guard(plain);
    let parenthesized_hover = past_guard(parenthesized);

    assert!(
        !plain_hover.contains("none"),
        "the guard throws on NONE, so nothing past it can be NONE; hover said: {plain_hover}"
    );
    assert_eq!(
        plain_hover, parenthesized_hover,
        "a pair of parentheses is not a fact; the editor must report one kind for both spellings"
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
// Host files (embedded SurrealQL)
// ---------------------------------------------------------------------------

/// A SvelteKit route with one bad query inside a `db.query` literal.
const HOST_URI: &str = "file:///workspace/src/routes/+page.svelte";
const HOST: &str = "\
<script lang=\"ts\">
  import { db } from '$lib/db';

  const q = db.query(\"SELECT username, nonExistent FROM account\");
</script>

<h1>hello</h1>
";

/// The host text an LSP range covers — the proof a squiggle lands on the
/// offending token and not on the string, the call, or the line.
fn text_at(text: &str, range: &Value) -> String {
    let line_start = |line: usize| {
        text.split_inclusive('\n')
            .take(line)
            .map(str::len)
            .sum::<usize>()
    };
    let at = |end: &Value| {
        line_start(end["line"].as_u64().expect("line") as usize)
            + end["character"].as_u64().expect("character") as usize
    };
    text[at(&range["start"])..at(&range["end"])].to_string()
}

#[test]
fn a_bad_query_in_a_svelte_file_is_flagged_on_the_offending_token() {
    let mut lsp = Lsp::start();
    let _ = lsp.did_open(SCHEMA_URI, SCHEMA);
    lsp.notify(
        "textDocument/didOpen",
        json!({"textDocument": {
            "uri": HOST_URI, "languageId": "svelte", "version": 1, "text": HOST,
        }}),
    );
    let diagnostics = lsp.next_publish(HOST_URI);

    let unknown_field = diagnostics
        .iter()
        .find(|d| d["code"] == "E1002")
        .unwrap_or_else(|| panic!("expected E1002 in the embedded query, got: {diagnostics:?}"));
    assert_eq!(unknown_field["severity"], 1);
    // The range is in HOST coordinates, and covers exactly the field name
    // inside the template literal.
    assert_eq!(
        text_at(HOST, &unknown_field["range"]),
        "nonExistent",
        "the squiggle covers the offending token, not the whole template"
    );
    assert_eq!(unknown_field["range"]["start"]["line"], 3);
}

#[test]
fn a_host_file_the_schema_satisfies_publishes_nothing() {
    let mut lsp = Lsp::start();
    let _ = lsp.did_open(SCHEMA_URI, SCHEMA);

    // A clean query, and a host file with no SurrealQL in it at all: neither
    // may put a mark in the editor.
    for (uri, text) in [
        (
            "file:///workspace/src/clean.ts",
            "const q = db.query(\"SELECT username FROM account\");\n",
        ),
        (
            "file:///workspace/src/plain.ts",
            "export const answer = 42;\n",
        ),
    ] {
        lsp.notify(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": uri, "languageId": "typescript", "version": 1, "text": text,
            }}),
        );
        assert_eq!(
            lsp.next_publish(uri),
            Vec::<Value>::new(),
            "{uri} must publish nothing"
        );
    }
}

#[test]
fn a_template_substitution_is_a_parameter_not_an_unknown_name() {
    // `${...}` becomes a `$__hostN` parameter. A parameter the analyzer cannot
    // resolve is a real finding class, so this asserts the rewrite does not
    // manufacture one out of ordinary interpolation.
    let mut lsp = Lsp::start();
    let _ = lsp.did_open(SCHEMA_URI, SCHEMA);
    let uri = "file:///workspace/src/interpolated.ts";
    let text = "const name = 'ada';\n\
                const q = db.query(`SELECT username FROM account WHERE username = ${name}`);\n";
    lsp.notify(
        "textDocument/didOpen",
        json!({"textDocument": {
            "uri": uri, "languageId": "typescript", "version": 1, "text": text,
        }}),
    );
    assert_eq!(lsp.next_publish(uri), Vec::<Value>::new());
}

#[test]
fn hover_inside_an_embedded_query_answers_as_the_query() {
    let mut lsp = Lsp::start();
    let _ = lsp.did_open(SCHEMA_URI, SCHEMA);
    lsp.notify(
        "textDocument/didOpen",
        json!({"textDocument": {
            "uri": HOST_URI, "languageId": "svelte", "version": 1, "text": HOST,
        }}),
    );
    let _ = lsp.next_publish(HOST_URI);

    let cursor = HOST.find("username").expect("the field is in the template") + 2;
    let result = lsp.request(
        "textDocument/hover",
        json!({"textDocument": {"uri": HOST_URI}, "position": position_of(HOST, cursor)}),
    );
    let markdown = result["contents"]["value"]
        .as_str()
        .unwrap_or_else(|| panic!("expected hover inside the embedded query, got: {result}"));
    assert!(
        markdown.contains("string"),
        "hover should report the field's declared type, got: {markdown}"
    );
    // And it points at the host token, not at an offset into the query text.
    assert_eq!(text_at(HOST, &result["range"]), "username");

    // Hovering the surrounding host language is not ours to answer.
    let outside = HOST.find("import").expect("host code");
    let result = lsp.request(
        "textDocument/hover",
        json!({"textDocument": {"uri": HOST_URI}, "position": position_of(HOST, outside)}),
    );
    assert!(
        result.is_null(),
        "expected no hover in host code, got {result}"
    );
}

// ---------------------------------------------------------------------------
// Semantic tokens
// ---------------------------------------------------------------------------

/// Decodes a `semanticTokens/full` response back into `(covered text, token
/// type)` pairs. The delta encoding is exactly where a highlighting bug hides,
/// so the assertions are on the text the editor would actually paint.
fn decode_tokens(text: &str, legend: &[Value], data: &[u64]) -> Vec<(String, String)> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut decoded = Vec::new();
    let (mut line, mut start) = (0usize, 0usize);
    for token in data.chunks(5) {
        let [delta_line, delta_start, length, token_type, _modifiers] = token else {
            panic!("semantic token data comes in fives, got {token:?}");
        };
        line += *delta_line as usize;
        start = if *delta_line == 0 {
            start + *delta_start as usize
        } else {
            *delta_start as usize
        };
        // The corpus is ASCII, so a UTF-16 column is a byte column.
        let covered = &lines[line][start..start + *length as usize];
        decoded.push((
            covered.to_string(),
            legend[*token_type as usize]
                .as_str()
                .expect("legend entry")
                .to_string(),
        ));
    }
    decoded
}

#[test]
fn a_query_inside_a_svelte_file_comes_back_syntax_highlighted() {
    let mut lsp = Lsp::start();
    let legend = lsp.token_legend();
    let _ = lsp.did_open(SCHEMA_URI, SCHEMA);
    lsp.notify(
        "textDocument/didOpen",
        json!({"textDocument": {
            "uri": HOST_URI, "languageId": "svelte", "version": 1, "text": HOST,
        }}),
    );
    let _ = lsp.next_publish(HOST_URI);

    let result = lsp.request(
        "textDocument/semanticTokens/full",
        json!({"textDocument": {"uri": HOST_URI}}),
    );
    let data: Vec<u64> = result["data"]
        .as_array()
        .unwrap_or_else(|| panic!("expected semantic tokens, got: {result}"))
        .iter()
        .map(|value| value.as_u64().expect("token datum"))
        .collect();

    // Only the query is tokenized — the `import` line and the markup around
    // it belong to whichever server owns Svelte.
    assert_eq!(
        decode_tokens(HOST, &legend, &data),
        vec![
            ("SELECT".to_string(), "keyword".to_string()),
            ("username".to_string(), "variable".to_string()),
            ("nonExistent".to_string(), "variable".to_string()),
            ("FROM".to_string(), "keyword".to_string()),
            ("account".to_string(), "variable".to_string()),
        ]
    );
}

#[test]
fn semantic_tokens_cover_a_surql_file_the_same_way() {
    let mut lsp = Lsp::start();
    let legend = lsp.token_legend();
    let query = "SELECT username FROM account WHERE username = $name;\n";
    let _ = lsp.did_open(SCHEMA_URI, SCHEMA);
    let _ = lsp.did_open(QUERY_URI, query);

    let result = lsp.request(
        "textDocument/semanticTokens/full",
        json!({"textDocument": {"uri": QUERY_URI}}),
    );
    let data: Vec<u64> = result["data"]
        .as_array()
        .unwrap_or_else(|| panic!("expected semantic tokens, got: {result}"))
        .iter()
        .map(|value| value.as_u64().expect("token datum"))
        .collect();
    let decoded = decode_tokens(query, &legend, &data);

    assert_eq!(
        decoded.first().map(|(text, _)| text.as_str()),
        Some("SELECT")
    );
    assert!(
        decoded.contains(&("$name".to_string(), "parameter".to_string())),
        "a parameter is a parameter in a .surql file too, got: {decoded:?}"
    );
}

// ---------------------------------------------------------------------------
// Semantic-token refresh (server→client requests)
// ---------------------------------------------------------------------------

/// The client capability that says "I will honor `workspace/semanticTokens/
/// refresh`".
fn refresh_capable() -> Value {
    json!({"workspace": {"semanticTokens": {"refreshSupport": true}}})
}

#[test]
fn no_semantic_token_refresh_is_sent_to_a_client_that_did_not_ask_for_one() {
    // `Lsp::start` advertises no capabilities at all. Every publish used to
    // send a refresh regardless; a client that ignores unknown requests then
    // leaked one pending entry per keystroke inside the server, forever.
    let (mut lsp, _) = Lsp::with_schema("SELECT name FROM organization;\n");
    for version in 2..6 {
        let _ = lsp.did_change(
            QUERY_URI,
            version,
            &format!("SELECT name FROM organization; -- {version}\n"),
        );
    }
    lsp.settle();

    assert_eq!(
        lsp.refresh_requests(),
        0,
        "a refresh must never reach a client that did not advertise \
         `workspace.semanticTokens.refreshSupport`; server requests seen: {:?}",
        lsp.server_requests
    );
}

#[test]
fn a_client_that_never_answers_a_refresh_is_sent_at_most_one() {
    // The client advertised refresh support but never replies (a stalled or
    // careless editor). Every publish asks for a refresh; the server must
    // fold them onto the one already in flight rather than queue another.
    let mut lsp = Lsp::start_as(refresh_capable(), false);
    let _ = lsp.did_open(SCHEMA_URI, SCHEMA);
    let _ = lsp.did_open(QUERY_URI, "SELECT name FROM organization;\n");
    for version in 2..8 {
        let _ = lsp.did_change(
            QUERY_URI,
            version,
            &format!("SELECT name FROM organization; -- {version}\n"),
        );
    }
    lsp.settle();

    assert_eq!(
        lsp.refresh_requests(),
        1,
        "eight publishes with the first refresh still unanswered must send \
         exactly one refresh, got {:?}",
        lsp.server_requests
    );
}

#[test]
fn a_client_that_answers_refreshes_gets_one_per_burst_and_never_more_than_one_per_publish() {
    let mut lsp = Lsp::start_as(refresh_capable(), true);
    // The `initialized` sweep asks once even before anything is open; each
    // `didOpen` and `didChange` publishes (and may ask) once more.
    let mut publishes = 1;
    let _ = lsp.did_open(SCHEMA_URI, SCHEMA);
    let _ = lsp.did_open(QUERY_URI, "SELECT name FROM organization;\n");
    publishes += 2;
    for version in 2..8 {
        let _ = lsp.did_change(
            QUERY_URI,
            version,
            &format!("SELECT name FROM organization; -- {version}\n"),
        );
        publishes += 1;
    }
    lsp.settle();

    let refreshes = lsp.refresh_requests();
    assert!(
        (1..=publishes).contains(&refreshes),
        "an answering client must be told to re-pull at least once and at most \
         once per publish or sweep ({publishes}), got {refreshes}: {:?}",
        lsp.server_requests
    );
}

// ---------------------------------------------------------------------------
// Code actions: suppress here / suppress workspace-wide
// ---------------------------------------------------------------------------

/// The schema the code-action fixtures resolve against, on disk in the root.
const ROOT_SCHEMA: &str = "\
DEFINE TABLE account SCHEMAFULL;
DEFINE FIELD username ON account TYPE string;
";

/// A `surrealql-analyzer.toml` written by hand: leading comment, an existing
/// `[lints]` table with comments *inside* it, and another table after it.
/// Mangling any of that would be the visible failure.
const COMMENTED_CONFIG: &str = "\
# SurrealQL Analyzer workspace config for the demo app.

[sources]
# .svelte-kit holds generated route types; never scan it.
ignore = [\"node_modules/**\", \".svelte-kit/**\"]

[lints]
# Everything stylistic stays advisory in this workspace.
\"7xxx\" = \"warn\"

[analysis]
strict = false
";

fn diagnostic_with_code<'a>(diagnostics: &'a [Value], code: &str) -> &'a Value {
    diagnostics
        .iter()
        .find(|diagnostic| diagnostic["code"] == code)
        .unwrap_or_else(|| panic!("expected a {code} diagnostic, got: {diagnostics:?}"))
}

#[test]
fn a_surql_diagnostic_offers_both_suppressions_and_the_inline_one_actually_silences_it() {
    let root = TempRoot::new("surql");
    root.write("a_schema.surql", ROOT_SCHEMA);
    // Indented on purpose: the inserted directive has to line up with the
    // statement it covers, not sit at column zero above it.
    let query = "BEGIN;\n    SELECT * FROM persn;\nCOMMIT;\n";
    let query_uri = root.write("b_query.surql", query);
    root.write("surrealql-analyzer.toml", "[analysis]\nstrict = false\n");

    let mut lsp = Lsp::start_in(&root.path);
    assert!(
        lsp.capabilities["codeActionProvider"].is_object(),
        "a client that declared codeActionLiteralSupport must be offered the \
         provider, got: {}",
        lsp.capabilities
    );

    let diagnostics = lsp.did_open(&query_uri, query);
    let unknown_table = diagnostic_with_code(&diagnostics, "E1001").clone();
    let actions = lsp.code_actions(&query_uri, &unknown_table);

    assert_eq!(
        titles(&actions),
        vec![
            "Suppress E1001 here".to_string(),
            "Suppress E1001 workspace-wide (surrealql-analyzer.toml)".to_string(),
        ],
        "got: {actions:?}"
    );
    for action in &actions {
        assert_eq!(action["kind"], "quickfix");
        assert_eq!(action["diagnostics"][0]["code"], "E1001");
    }

    // The inline edit, applied.
    let (uri, edit) = sole_edit(&actions[0]);
    assert_eq!(uri, query_uri, "the inline edit belongs to the query file");
    let suppressed = apply_lsp_edit(query, &edit);
    assert_eq!(
        suppressed,
        "BEGIN;\n    -- surrealql-analyzer: allow(E1001)\n    SELECT * FROM persn;\nCOMMIT;\n"
    );

    // Some clients send an empty context when the request comes from a
    // keybinding rather than a lightbulb; the same actions must still appear.
    let bare = lsp.request(
        "textDocument/codeAction",
        json!({
            "textDocument": {"uri": query_uri},
            "range": unknown_table["range"],
            "context": {"diagnostics": [], "triggerKind": 1},
        }),
    );
    assert_eq!(
        titles(bare.as_array().expect("an action list")),
        titles(&actions),
        "an empty context must fall back to our own findings at that range"
    );

    // Round trip: the directive the action wrote really does silence it.
    let after = lsp.did_change(&query_uri, 2, &suppressed);
    assert!(
        !after.iter().any(|d| d["code"] == "E1001"),
        "the suppressed finding must be gone, got: {after:?}"
    );
    assert!(
        !after.iter().any(|d| d["code"].as_str() == Some("W7013")),
        "the directive must not itself be a malformed-directive finding: {after:?}"
    );
}

#[test]
fn a_workspace_requiring_reasons_gets_a_directive_carrying_one() {
    let root = TempRoot::new("reasons");
    root.write("a_schema.surql", ROOT_SCHEMA);
    let query = "SELECT * FROM persn;\n";
    let query_uri = root.write("b_query.surql", query);
    root.write(
        "surrealql-analyzer.toml",
        "[diagnostics]\nrequire_suppression_reasons = true\n",
    );

    let mut lsp = Lsp::start_in(&root.path);
    let diagnostics = lsp.did_open(&query_uri, query);
    let unknown_table = diagnostic_with_code(&diagnostics, "E1001").clone();
    let actions = lsp.code_actions(&query_uri, &unknown_table);

    let (_, edit) = sole_edit(&actions[0]);
    let suppressed = apply_lsp_edit(query, &edit);
    assert_eq!(
        suppressed,
        "-- surrealql-analyzer: allow(E1001) reason=\"TODO: explain why this is allowed\"\n\
         SELECT * FROM persn;\n"
    );

    let after = lsp.did_change(&query_uri, 2, &suppressed);
    assert!(
        after.is_empty(),
        "a reason-carrying directive suppresses cleanly in a workspace that \
         requires one, got: {after:?}"
    );
}

#[test]
fn a_single_line_host_string_gets_the_workspace_action_only_and_it_works() {
    let root = TempRoot::new("svelte");
    root.write("a_schema.surql", ROOT_SCHEMA);
    root.write("surrealql-analyzer.toml", COMMENTED_CONFIG);
    // The demo shape: the query is inside a single-line attribute string. A
    // `--` comment cannot be put anywhere in it without breaking the file.
    let page = "<Query q=\"SELECT * FROM persn\" />\n";
    let page_uri = root.write("Page.svelte", page);

    let mut lsp = Lsp::start_in(&root.path);
    let diagnostics = lsp.did_open(&page_uri, page);
    let unknown_table = diagnostic_with_code(&diagnostics, "E1001").clone();
    let actions = lsp.code_actions(&page_uri, &unknown_table);

    assert_eq!(
        titles(&actions),
        vec!["Suppress E1001 workspace-wide (surrealql-analyzer.toml)".to_string()],
        "an inline directive would corrupt this file, so it must not be \
         offered here. Got: {actions:?}"
    );

    let (config_uri, edit) = sole_edit(&actions[0]);
    assert_eq!(
        config_uri,
        file_uri(&root.path.join("surrealql-analyzer.toml"))
    );
    let edited = apply_lsp_edit(COMMENTED_CONFIG, &edit);
    assert_eq!(
        edited,
        "\
# SurrealQL Analyzer workspace config for the demo app.

[sources]
# .svelte-kit holds generated route types; never scan it.
ignore = [\"node_modules/**\", \".svelte-kit/**\"]

[lints]
# Everything stylistic stays advisory in this workspace.
\"7xxx\" = \"warn\"
E1001 = \"allow\"

[analysis]
strict = false
",
        "every comment and every existing entry must survive verbatim"
    );

    // Round trip: write what the client would have written, tell the server
    // the way a client with file watching would, and the finding is gone.
    root.write("surrealql-analyzer.toml", &edited);
    lsp.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": config_uri, "type": 2}]}),
    );
    let after = lsp.next_publish(&page_uri);
    assert!(
        !after.iter().any(|d| d["code"] == "E1001"),
        "an allowed code must stop being published, got: {after:?}"
    );
    assert_eq!(
        root.read("Page.svelte"),
        page,
        "the host file itself is never touched by the workspace action"
    );
}

#[test]
fn a_multi_line_template_takes_an_inline_directive_that_silences_it() {
    let root = TempRoot::new("template");
    root.write("a_schema.surql", ROOT_SCHEMA);
    root.write("surrealql-analyzer.toml", COMMENTED_CONFIG);
    // A backtick template that already spans lines: column zero of the
    // diagnostic's line is query text, so a directive line is safe there.
    let module = "const rows = await db.query(`\n  SELECT * FROM persn;\n`);\n";
    let module_uri = root.write("queries.ts", module);

    let mut lsp = Lsp::start_in(&root.path);
    let diagnostics = lsp.did_open(&module_uri, module);
    let unknown_table = diagnostic_with_code(&diagnostics, "E1001").clone();
    let actions = lsp.code_actions(&module_uri, &unknown_table);

    assert_eq!(
        titles(&actions),
        vec![
            "Suppress E1001 here".to_string(),
            "Suppress E1001 workspace-wide (surrealql-analyzer.toml)".to_string(),
        ],
        "got: {actions:?}"
    );

    let (uri, edit) = sole_edit(&actions[0]);
    assert_eq!(uri, module_uri);
    let suppressed = apply_lsp_edit(module, &edit);
    assert_eq!(
        suppressed,
        "const rows = await db.query(`\n  -- surrealql-analyzer: allow(E1001)\n  \
         SELECT * FROM persn;\n`);\n"
    );

    let after = lsp.did_change(&module_uri, 2, &suppressed);
    assert!(
        !after.iter().any(|d| d["code"] == "E1001"),
        "the directive must silence the embedded finding too, got: {after:?}"
    );
}

/// The footgun this feature exists to not step in: a lint is *displayed* as
/// `W7015` (its resolved severity) but *suppressed* as `L7015` (its category).
/// Copying what the editor showed would produce a directive that parses,
/// reads plausibly, and silences nothing at all.
#[test]
fn a_lint_is_suppressed_by_its_category_code_not_the_one_the_editor_displays() {
    let root = TempRoot::new("lint");
    root.write("a_schema.surql", ROOT_SCHEMA);
    let query = "SELECT * FROM account;\n";
    let query_uri = root.write("b_query.surql", query);
    // 7015 is off by default; turn it on so the editor publishes it as W7015.
    root.write("surrealql-analyzer.toml", "[lints]\n7015 = \"warn\"\n");

    let mut lsp = Lsp::start_in(&root.path);
    let diagnostics = lsp.did_open(&query_uri, query);
    let select_star = diagnostic_with_code(&diagnostics, "W7015").clone();
    let actions = lsp.code_actions(&query_uri, &select_star);

    assert_eq!(
        titles(&actions),
        vec![
            "Suppress W7015 here".to_string(),
            "Suppress W7015 workspace-wide (surrealql-analyzer.toml)".to_string(),
        ],
        "the title names the code the user sees; the edit must not. Got: {actions:?}"
    );

    let (_, inline) = sole_edit(&actions[0]);
    let suppressed = apply_lsp_edit(query, &inline);
    assert_eq!(
        suppressed, "-- surrealql-analyzer: allow(L7015)\nSELECT * FROM account;\n",
        "`allow(W7015)` would parse and suppress nothing"
    );
    let after = lsp.did_change(&query_uri, 2, &suppressed);
    assert!(
        !after.iter().any(|d| d["code"] == "W7015"),
        "got: {after:?}"
    );

    // The existing entry is re-levelled in place rather than duplicated —
    // TOML rejects a table with the same key twice.
    let (_, in_config) = sole_edit(&actions[1]);
    assert_eq!(
        apply_lsp_edit("[lints]\n7015 = \"warn\"\n", &in_config),
        "[lints]\n7015 = \"allow\"\n"
    );
}

#[test]
fn a_client_that_never_asked_for_code_actions_is_offered_none() {
    // `Lsp::start` handshakes with empty capabilities — no
    // codeActionLiteralSupport, so the server must advertise no provider and
    // answer the request with nothing rather than a shape the client cannot
    // read.
    let (mut lsp, _) = Lsp::with_schema("SELECT * FROM persn;\n");
    assert!(
        lsp.capabilities["codeActionProvider"].is_null(),
        "got: {}",
        lsp.capabilities
    );

    let result = lsp.request(
        "textDocument/codeAction",
        json!({
            "textDocument": {"uri": QUERY_URI},
            "range": {"start": {"line": 0, "character": 14},
                      "end": {"line": 0, "character": 19}},
            "context": {"diagnostics": [], "triggerKind": 1},
        }),
    );
    assert!(result.is_null(), "got: {result}");
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
