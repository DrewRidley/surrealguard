# SurrealGuard v3 Foundation Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Build the v3 foundation described in `docs/DESIGN.md`: a tree-sitter-based SurrealQL semantic intelligence engine with stable spans, diagnostics, lints, owned types, workspace analysis, LSP/CLI/MCP surfaces, and a first embedded-query bridge spike.

**Architecture:** Treat the current analyzer/core/codegen/CLI implementation as reference material, not as a compatibility contract. Add syntax/source/diagnostic/type/workspace contracts first, then route CLI/LSP/MCP and host adapters through the same `AnalysisOutput` model. Quarantine or delete legacy paths when they slow the v3 design; only re-adopt old behavior deliberately.

**Tech Stack:** Rust workspace, tree-sitter SurrealQL grammar, tower-lsp, serde/serde_json, TOML config, focused v3 tests, old analyzer tests as reference cases only, future MCP server crate.

---

## Ground rules

- This is a ground-up rewrite. Do not preserve old analyzer behavior by default.
- Do not commit `.DS_Store` files.
- Quarantine or delete legacy paths once they block clarity, compile gates, or the new crate boundaries.
- Use focused v3 crate tests as the correctness gate and `cargo check`/maintained-workspace checks as compile gates. Do not use old analyzer tests as a required regression gate.
- Prefer small commits that each leave the v3 workspace path compiling.
- When a public type or JSON shape is introduced, add tests for it immediately.
- `docs/DESIGN.md` is the source of truth. If this plan conflicts with it, update the plan or the design doc before coding.
- Treat finding classification as a foundational product contract, not a renderer detail. The rebuild starts around stable finding codes, category families, configurable lint policy, suppressions, and structured data so LSP, CLI/CI, MCP, and host adapters all consume the same semantics.

## Baseline commands

Run before starting implementation:

```bash
git status --short
git branch --show-current
cargo test -p surrealguard-syntax -p surrealguard-diagnostics -- --nocapture
cargo check --workspace
```

Expected baseline from design pass:

```text
branch: redesign-v3-foundation
known dirty files: .DS_Store, crates/.DS_Store, docs/DESIGN.md, docs/plans/v3-foundation.md
cargo focused v3 tests: passing
cargo check --workspace: passing
legacy analyzer tests: reference-only, not a gate
```

Before any commit:

```bash
git status --short
git add docs/DESIGN.md docs/plans/v3-foundation.md [intentional source files]
git diff --cached --name-only
```

Expected: no `.DS_Store` paths in staged files.

---

## Phase 0: Documentation checkpoint

### Task 0.1: Commit design and plan docs only

**Objective:** Create a clean documentation checkpoint before code changes.

**Files:**
- Stage: `docs/DESIGN.md`
- Stage: `docs/plans/v3-foundation.md`
- Do not stage: `.DS_Store`
- Do not stage: `crates/.DS_Store`

**Step 1: Verify status**

Run:

```bash
git status --short
```

Expected: dirty `.DS_Store` files plus untracked docs.

**Step 2: Stage only docs**

Run:

```bash
git add docs/DESIGN.md docs/plans/v3-foundation.md
git diff --cached --name-only
```

Expected:

```text
docs/DESIGN.md
docs/plans/v3-foundation.md
```

**Step 3: Commit**

Run:

```bash
git commit -m "docs: define SurrealGuard v3 foundation"
```

Expected: commit succeeds, `.DS_Store` remains unstaged.

---

## Phase 1: Syntax facade around tree-sitter

### Task 1.1: Create syntax crate skeleton

**Objective:** Introduce a dedicated syntax layer without changing analyzer behavior.

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/syntax/Cargo.toml`
- Create: `crates/syntax/src/lib.rs`
- Create: `crates/syntax/src/span.rs`
- Create: `crates/syntax/src/source.rs`
- Create: `crates/syntax/src/parse.rs`

**Step 1: Add workspace member**

Modify root `Cargo.toml` and add `crates/syntax` to workspace members.

**Step 2: Create crate manifest**

`crates/syntax/Cargo.toml` should start minimal:

```toml
[package]
name = "surrealguard-syntax"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { version = "1", features = ["derive"] }
tree-sitter = "0.24"
```

Adjust `tree-sitter` version to match the workspace if another version is already in use.

**Step 3: Add public modules**

`crates/syntax/src/lib.rs`:

```rust
pub mod parse;
pub mod source;
pub mod span;
```

**Step 4: Verify compile**

Run:

```bash
cargo check -p surrealguard-syntax
```

Expected: passes.

**Step 5: Commit**

```bash
git add Cargo.toml crates/syntax
git commit -m "feat: add syntax crate skeleton"
```

### Task 1.2: Add source and byte span primitives

**Objective:** Create reusable source/span primitives before touching analyzer diagnostics.

**Files:**
- Modify: `crates/syntax/src/source.rs`
- Modify: `crates/syntax/src/span.rs`
- Modify: `crates/syntax/src/lib.rs`

**Step 1: Implement primitives**

Add:

```rust
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceId(String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ByteRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceSpan {
    pub source: SourceId,
    pub range: ByteRange,
}
```

Add constructors that validate `start <= end`.

**Step 2: Add tests**

Add unit tests for:

- `ByteRange::new(0, 3)` succeeds
- `ByteRange::new(3, 0)` fails or panics consistently
- `ByteRange::len()` returns expected byte length
- `SourceSpan` preserves `SourceId`

**Step 3: Verify**

Run:

```bash
cargo test -p surrealguard-syntax span source
```

Expected: new tests pass.

**Step 4: Commit**

```bash
git add crates/syntax
git commit -m "feat: add source span primitives"
```

### Task 1.3: Add parsed source and syntax anchor types

**Objective:** Represent parsed files and syntax anchors without leaking tree-sitter lifetimes into semantic data.

**Files:**
- Modify: `crates/syntax/src/parse.rs`
- Modify: `crates/syntax/src/lib.rs`

**Step 1: Add types**

Implement:

```rust
use std::sync::Arc;
use tree_sitter::Tree;

use crate::source::SourceId;
use crate::span::{ByteRange, SourceSpan};

pub struct ParsedSource {
    pub source_id: SourceId,
    pub text: Arc<str>,
    pub tree: Tree,
    pub errors: Vec<SyntaxDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxAnchor {
    pub source: SourceId,
    pub byte_range: ByteRange,
    pub node_kind: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxDiagnostic {
    pub span: SourceSpan,
    pub message: String,
}
```

If `&'static str` is inconvenient for tree-sitter node kinds, use `SmolStr` or `String` and update `docs/DESIGN.md` later.

**Step 2: Add anchor tests**

Test that `SyntaxAnchor` is cloneable and does not borrow a tree.

**Step 3: Verify**

Run:

```bash
cargo test -p surrealguard-syntax
```

Expected: passes.

**Step 4: Commit**

```bash
git add crates/syntax
git commit -m "feat: add parsed source syntax anchors"
```

### Task 1.4: Wrap existing tree-sitter parser entry point

**Objective:** Route parsing through `surrealguard-syntax` while preserving current parser behavior.

**Files:**
- Modify: `crates/syntax/Cargo.toml`
- Modify: `crates/syntax/src/parse.rs`
- Modify: `crates/analyzer/Cargo.toml`
- Modify: current analyzer parser file, likely `crates/analyzer/src/parser.rs`

**Step 1: Locate current grammar dependency**

Search:

```bash
rg "tree_sitter|tree-sitter|surrealql" crates Cargo.toml
```

Use the existing maintained tree-sitter SurrealQL grammar dependency if already present.

**Step 2: Add parse function**

Expose a syntax API like:

```rust
pub fn parse_source(source_id: SourceId, text: impl Into<Arc<str>>) -> ParsedSource
```

It should:

- parse text with tree-sitter SurrealQL grammar
- collect tree-sitter error nodes into `SyntaxDiagnostic`
- keep the partial tree

**Step 3: Decide whether to keep any legacy API shim**

If a legacy API shim helps during the transition, keep it thin and mark it legacy. Do not contort the v3 syntax facade to preserve old call shapes.

**Step 4: Verify**

Run:

```bash
cargo check --workspace
```

Expected: maintained workspace crates compile. Legacy analyzer behavior tests are reference-only and are not a gate.

**Step 5: Commit**

```bash
git add crates/syntax crates/analyzer Cargo.toml
git commit -m "refactor: route parser through syntax facade"
```

---

## Phase 2: Stable diagnostics and lints

### Task 2.1: Add diagnostic code model

**Objective:** Introduce stable diagnostic codes without changing existing messages yet.

**Files:**
- Create or modify: `crates/analyzer/src/diagnostic.rs`
- Modify: `crates/analyzer/src/lib.rs`
- Modify tests as needed

**Step 1: Add code enums**

Implement:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiagnosticCode {
    Syntax(SyntaxCode),
    Schema(SchemaCode),
    Type(TypeCode),
    Param(ParamCode),
    Graph(GraphCode),
    Permission(PermissionCode),
    Dynamic(DynamicCode),
    Lint(LintCode),
}
```

Start with only codes needed by existing diagnostics plus the examples in `docs/DESIGN.md`.

**Step 2: Add display methods**

Add:

```rust
impl DiagnosticCode {
    pub fn number(self) -> &'static str;
    pub fn name(self) -> &'static str;
}
```

Examples:

- `E1001`, `schema.unknown_table`
- `E2001`, `type.mismatch`

**Step 3: Add tests**

Test stable number and name strings.

**Step 4: Verify**

```bash
cargo test -p surrealguard-analyzer diagnostic_code
```

Expected: passes.

**Step 5: Commit**

```bash
git add crates/analyzer
git commit -m "feat: add stable diagnostic codes"
```

### Task 2.2: Add structured diagnostic payload

**Objective:** Make diagnostics useful for LSP/CLI/MCP/adapters without parsing human messages.

**Files:**
- Modify: `crates/analyzer/src/diagnostic.rs`
- Modify diagnostics construction sites
- Modify tests

**Step 1: Extend diagnostic struct**

Add fields matching design:

```rust
pub struct Diagnostic {
    pub span: SourceSpan,
    pub severity: Severity,
    pub code: DiagnosticCode,
    pub message: String,
    pub help: Vec<Help>,
    pub related: Vec<RelatedInfo>,
    pub tags: Vec<DiagnosticTag>,
    pub data: DiagnosticData,
}
```

If current code uses a different `Span`, do not add broad compatibility conversions by default. Convert only at explicit legacy shims, or quarantine the legacy caller.

**Step 2: Add structured data enum**

Start small:

```rust
pub enum DiagnosticData {
    None,
    UnknownTable { table: String },
    UnknownField { table: Option<String>, field: String },
    TypeMismatch { expected: Type, found: Type },
}
```

Use boxed or stringified types temporarily if owned `Type` is not ready.

**Step 3: Keep render text stable only for new diagnostic contracts**

Do not rewrite every message in this task. Add structured payload and snapshot only the new diagnostic contracts; old human-output tests are reference-only.

**Step 4: Verify**

```bash
cargo test -p surrealguard-diagnostics
cargo check --workspace
```

Expected: passes.

**Step 5: Commit**

```bash
git add crates/analyzer
git commit -m "feat: add structured diagnostics"
```

### Task 2.3: Add lint config model

**Objective:** Add `allow`/`warn`/`deny` configuration without changing analyzer behavior broadly.

**Files:**
- Create or modify: `crates/workspace/src/config.rs` if workspace crate exists, otherwise temporary location under `crates/analyzer/src/config.rs`
- Modify manifests for `serde`/`toml` if needed
- Add tests

**Step 1: Add config types**

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LintLevel {
    Allow,
    Warn,
    Deny,
}

#[derive(Clone, Debug, Default)]
pub struct DiagnosticConfig {
    pub warnings_as_errors: bool,
    pub require_suppression_reasons: bool,
}

#[derive(Clone, Debug, Default)]
pub struct LintConfig {
    pub select_star: Option<LintLevel>,
    pub dynamic_query: Option<LintLevel>,
    pub permission_gated_field: Option<LintLevel>,
}
```

**Step 2: Add TOML parse test**

Test parsing:

```toml
[diagnostics]
warnings_as_errors = true
require_suppression_reasons = true

[lints]
select_star = "warn"
dynamic_query = "deny"
permission_gated_field = "warn"
```

**Step 3: Verify**

```bash
cargo test --workspace lint_config
```

Expected: passes.

**Step 4: Commit**

```bash
git add crates
git commit -m "feat: add diagnostic lint config"
```

### Task 2.4: Add suppression parser

**Objective:** Parse explicit suppressions in `.surql` comments and host comments.

**Files:**
- Create or modify: `crates/diagnostics/src/suppression.rs`
- Modify: `crates/diagnostics/src/lib.rs`
- Add tests

**Step 1: Implement parser**

Recognize forms:

```text
surrealguard: allow(lint.select_star) reason="..."
surrealguard: allow(dynamic.table_name) reason="..."
```

Return:

```rust
pub struct Suppression {
    pub code_or_name: String,
    pub reason: Option<String>,
    pub span: SourceSpan,
}
```

**Step 2: Add tests**

Test:

- valid suppression with reason
- valid suppression without reason
- invalid blanket suppression is rejected
- unknown directive is ignored or reported consistently

**Step 3: Verify**

```bash
cargo test -p surrealguard-diagnostics suppression
```

Expected: passes.

**Step 4: Commit**

```bash
git add crates/diagnostics docs/plans/v3-foundation.md
git commit -m "feat: parse surrealguard suppressions"
```

---

## Phase 3: Owned type model and generic signatures

### Task 3.1: Add owned type model crate

**Objective:** Introduce SurrealGuard-owned type representation as the v3 semantic contract. Existing `surrealdb::sql::Kind` usage is legacy and should not shape the public model.

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/types/Cargo.toml`
- Create: `crates/types/src/lib.rs`

**Step 1: Add core enum**

Implement the design shape with whatever small support structs are needed:

```rust
pub enum Type {
    Any,
    Unknown(UnknownReason),
    Never,
    None,
    Null,
    Bool,
    String,
    Number(NumberKind),
    Datetime,
    Duration,
    Uuid,
    Bytes,
    Regex,
    Range(Box<Type>),
    Array(Box<Type>, Option<ArrayLen>),
    Set(Box<Type>),
    Object(ObjectType),
    Record(TableSet),
    Geometry(GeometryKind),
    Optional(Box<Type>),
    Union(TypeSet),
    Literal(LiteralType),
    Generic(GenericType),
}
```

Use placeholder structs/enums as needed. Prefer `#[derive(Clone, Debug, PartialEq, Eq)]` where practical.

**Step 2: Add tests**

Test:

- `Any != Unknown`
- `Optional(String)` formats or compares predictably
- object fields preserve optional/readonly/permission metadata

**Step 3: Verify**

```bash
cargo test -p surrealguard-types
```

Expected: passes.

**Step 4: Commit**

```bash
git add Cargo.toml crates/types docs/plans/v3-foundation.md
git commit -m "feat: add owned SurrealGuard type model"
```

### Task 3.2: Add explicit optional import mapping from SurrealDB Kind

**Objective:** If useful for schema ingestion or differential tests, provide a narrow import mapping from SurrealDB `Kind` into the owned type model. This is not a compatibility layer for preserving the old analyzer.

**Files:**
- Modify: `crates/analyzer/src/type_model.rs`
- Modify: `crates/analyzer/src/types.rs`
- Add tests

**Step 1: Add import function**

Add an explicitly named import function from `surrealdb::sql::Kind` to `type_model::Type`, behind a module boundary that makes the dependency easy to remove later.

Important mapping decisions:

- SurrealDB `Kind::Any` maps to `Type::Any`, not `Unknown`
- current regex sentinel hack must map to `Type::Regex`
- unsupported or lossy cases map to `Unknown(reason)` with explicit reason

**Step 2: Add tests**

Test common kinds and the regex sentinel behavior.

**Step 3: Verify**

```bash
cargo test -p surrealguard-analyzer type_model
cargo check --workspace
```

Expected: passes.

**Step 4: Commit**

```bash
git add crates/analyzer
git commit -m "feat: add SurrealDB kind import mapping"
```

### Task 3.3: Add type compatibility module

**Objective:** Centralize type assignability as part of the owned type contract. Operator compatibility can build on this later when the semantic analyzer exists.

**Files:**
- Create: `crates/types/src/compatibility.rs`
- Modify: `crates/types/src/lib.rs`
- Add tests

**Step 1: Add API**

```rust
pub enum Compatibility {
    Assignable,
    NotAssignable,
    Unknown,
}

pub fn assignability(source: &Type, target: &Type) -> Compatibility;
pub fn is_assignable_to(source: &Type, target: &Type) -> bool;
```

**Step 2: Add focused tests**

Test:
- exact type equality
- `int` widens to `number`
- literals assign to primitive parents
- `optional<string>` accepts `none` and `string` but not `null`
- union source and union target behavior
- arrays, records, geometry, and structural objects
- `Unknown` propagates without pretending to be `Any`

**Step 3: Do not migrate all analyzer uses yet**

This task only creates and tests the owned type compatibility module. Legacy analyzer `Kind` compatibility remains reference material until explicitly replaced.

**Step 4: Verify**

```bash
cargo test -p surrealguard-types compatibility
```

Expected: passes.

**Step 5: Commit**

```bash
git add crates/types
git commit -m "feat: add type assignability rules"
```

### Task 3.4: Add generic function signature model

**Objective:** Represent built-in functions with reusable generic signatures instead of ad hoc matching, using the owned type model rather than legacy analyzer types.

**Files:**
- Create: `crates/types/src/signature.rs`
- Modify: `crates/types/src/lib.rs`
- Add tests

**Step 1: Add signature types**

Implement:

```rust
pub struct FunctionSig {
    pub name: String,
    pub generics: Vec<GenericParam>,
    pub params: Vec<ParamType>,
    pub returns: TypeExpr,
}

pub enum ParamType {
    Type(TypeExpr),
    Variadic(Box<ParamType>),
}

pub enum TypeExpr {
    Exact(Type),
    Generic(String),
    Array(Box<TypeExpr>),
    Optional(Box<TypeExpr>),
    OneOf(Vec<TypeExpr>),
}
```

**Step 2: Add signature solver**

Start with simple local constraint solving:

- bind generic `T` from argument type
- reuse bound `T` in return type
- detect conflicting generic bindings
- use owned type assignability for exact parameters

**Step 3: Add tests**

Test examples:

- `array::len<T>(array<T>) -> int`
- `array::first<T>(array<T>) -> option<T>`
- repeated generic parameters reuse the same binding
- conflicting generic binding reports a structured `SignatureError`
- exact parameters use assignability rules

**Step 4: Verify**

```bash
cargo test -p surrealguard-types signature
```

Expected: passes.

**Step 5: Commit**

```bash
git add crates/types docs/plans/v3-foundation.md
git commit -m "feat: add generic function signature solver"
```

---

## Phase 4: Workspace layer

### Task 4.1: Create workspace crate skeleton

**Objective:** Introduce a workspace layer for config, source registry, schema assembly, and cross-file analysis.

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/workspace/Cargo.toml`
- Create: `crates/workspace/src/lib.rs`
- Create: `crates/workspace/src/config.rs`
- Create: `crates/workspace/src/source_registry.rs`

**Step 1: Add workspace member**

Add `crates/workspace` to root workspace members.

**Step 2: Add manifest**

Dependencies should include:

- `surrealguard-syntax`
- `surrealguard-analyzer`
- `serde`
- `toml`
- `globset` or existing workspace glob library

**Step 3: Add module skeleton**

Expose:

```rust
pub mod config;
pub mod source_registry;
```

**Step 4: Verify**

```bash
cargo check -p surrealguard-workspace
```

Expected: passes.

**Step 5: Commit**

```bash
git add Cargo.toml crates/workspace
git commit -m "feat: add workspace crate skeleton"
```

### Task 4.2: Implement source registry

**Objective:** Assign stable source ids and store text/line indexes in one place.

**Files:**
- Modify: `crates/workspace/src/source_registry.rs`
- Add tests

**Step 1: Implement registry**

API shape:

```rust
pub struct SourceRegistry { /* fields */ }

impl SourceRegistry {
    pub fn add_file(&mut self, path: PathBuf, text: String) -> SourceId;
    pub fn add_virtual(&mut self, name: String, text: String) -> SourceId;
    pub fn text(&self, source: &SourceId) -> Option<&str>;
    pub fn line_index(&self, source: &SourceId) -> Option<&LineIndex>;
}
```

**Step 2: Add byte-to-line/column support**

Keep canonical spans byte-based, but registry should support presentation mapping.

**Step 3: Add tests**

Test:

- same file path gets stable id within registry
- virtual sources get unique ids
- byte-to-line/column mapping handles multiline text

**Step 4: Verify**

```bash
cargo test -p surrealguard-workspace source_registry
```

Expected: passes.

**Step 5: Commit**

```bash
git add crates/workspace
git commit -m "feat: add workspace source registry"
```

### Task 4.3: Load surrealguard.toml

**Objective:** Implement project config loading with sensible defaults.

**Files:**
- Modify: `crates/workspace/src/config.rs`
- Add tests

**Step 1: Implement config shape**

Support:

```toml
[sources]
schema = ["schema/**/*.surql", "migrations/**/*.surql"]
queries = ["queries/**/*.surql"]

[analysis]
strict = false
surrealdb_version = "2"

[diagnostics]
warnings_as_errors = false
require_suppression_reasons = false

[lints]
select_star = "allow"
dynamic_query = "warn"
permission_gated_field = "warn"
```

**Step 2: Add default behavior**

No config should mean:

- include `.surql` and `.surrealql` files under root
- ignore dependency/build directories
- strict false

**Step 3: Add tests**

Test explicit config and no-config defaults.

**Step 4: Verify**

```bash
cargo test -p surrealguard-workspace config
```

Expected: passes.

**Step 5: Commit**

```bash
git add crates/workspace
git commit -m "feat: load surrealguard workspace config"
```

### Task 4.4: Add workspace analysis API

**Objective:** Provide the shared API that CLI, LSP, MCP, and adapters will call.

**Files:**
- Modify: `crates/workspace/src/lib.rs`
- Create: `crates/workspace/src/analysis.rs`
- Add tests

**Step 1: Add output shape**

```rust
pub struct AnalysisOutput {
    pub diagnostics: Vec<Diagnostic>,
    pub statements: Vec<StatementAnalysis>,
    pub inferred_params: Vec<ParamInference>,
    pub result_type: Option<Type>,
}
```

Use existing analyzer equivalents if they already exist, but expose a workspace-owned stable facade.

**Step 2: Add APIs**

```rust
pub fn analyze_source(workspace: &Workspace, source: SourceId) -> AnalysisOutput;
pub fn analyze_query(workspace: &Workspace, query_text: &str) -> AnalysisOutput;
pub fn analyze_workspace(workspace: &Workspace) -> WorkspaceAnalysis;
```

**Step 3: Add tests**

Use a tiny schema and query. Test that diagnostics flow through.

**Step 4: Verify**

```bash
cargo test -p surrealguard-workspace analysis
cargo check --workspace
```

Expected: passes.

**Step 5: Commit**

```bash
git add crates/workspace
git commit -m "feat: expose workspace analysis api"
```

---

## Phase 5: CLI, LSP, and MCP shared surfaces

### Task 5.1: Rebuild CLI check on workspace analysis

**Objective:** Make `surrealguard check` use the shared workspace analysis path.

**Files:**
- Modify: `crates/cli/Cargo.toml`
- Modify: `crates/cli/src/main.rs`
- Add integration tests if CLI tests exist

**Step 1: Add workspace crate dependency**

Depend on `surrealguard-workspace`.

**Step 2: Route check command**

`surrealguard check` should:

- discover/load config
- load source files
- call workspace analysis
- render diagnostics
- exit nonzero on errors or denied lints

**Step 3: Preserve existing CLI commands temporarily**

Legacy commands may be removed or quarantined once the v3 `check` path has its own contract tests and help output.

**Step 4: Verify**

```bash
cargo run -p surrealguard-cli -- check --help
cargo check --workspace
```

Expected: help works, tests pass.

**Step 5: Commit**

```bash
git add crates/cli
git commit -m "feat: route cli check through workspace analysis"
```

### Task 5.2: Add CLI JSON output

**Objective:** Provide machine-readable output for CI and agents.

**Files:**
- Modify: `crates/cli/src/main.rs`
- Add serialization derives in analyzer/workspace types as needed
- Add tests

**Step 1: Add flag**

Support:

```bash
surrealguard check --format human
surrealguard check --format json
```

Default: human.

**Step 2: JSON schema**

Output should include:

- source id/path
- byte spans
- severity
- code number
- code name
- message
- help
- related info
- structured data

**Step 3: Add snapshot or schema test**

Test JSON contains stable keys. Avoid brittle full-output snapshots unless existing project style prefers them.

**Step 4: Verify**

```bash
cargo test --workspace cli_json
```

Expected: passes.

**Step 5: Commit**

```bash
git add crates/cli crates/analyzer crates/workspace
git commit -m "feat: add cli json diagnostics"
```

### Task 5.3: Route LSP diagnostics through workspace API

**Objective:** Ensure LSP and CLI see the same diagnostics.

**Files:**
- Modify: `crates/lsp/Cargo.toml`
- Modify: `crates/lsp/src/backend.rs`
- Modify: `crates/lsp/src/workspace.rs` or remove duplication gradually
- Add tests if available

**Step 1: Add workspace crate dependency**

Depend on `surrealguard-workspace`.

**Step 2: Replace shadow schema analysis**

Route document analysis through `Workspace::analyze_source` or equivalent.

**Step 3: Map byte spans to LSP UTF-16 ranges**

Use source registry line index, not ad hoc conversion.

**Step 4: Verify**

```bash
cargo test -p surrealguard-lsp
cargo check --workspace
```

Expected: passes.

**Step 5: Commit**

```bash
git add crates/lsp
git commit -m "refactor: route lsp diagnostics through workspace analysis"
```

### Task 5.4: Add LSP hover and inlay from semantic model

**Objective:** Provide type hints and hover using the same analysis output.

**Files:**
- Modify: `crates/lsp/src/backend.rs`
- Modify or create semantic query helpers in `crates/workspace`
- Add tests if available

**Step 1: Add workspace semantic operations**

Implement:

```rust
hover(source_id, byte_offset) -> HoverPayload
inlay_hints(source_id, range) -> Vec<InlayHintPayload>
```

**Step 2: Implement LSP mapping**

Map payloads to LSP hover/inlay types.

**Step 3: Keep output concise**

Follow `docs/DX_SPEC.md`: no noisy metadata, precise type info, no keyword hovers.

**Step 4: Verify**

```bash
cargo test -p surrealguard-lsp hover inlay
cargo check --workspace
```

Expected: passes.

**Step 5: Commit**

```bash
git add crates/lsp crates/workspace
git commit -m "feat: add semantic hover and inlay hints"
```

### Task 5.5: Create MCP crate skeleton

**Objective:** Expose the same semantic model to LLM agents as structured tools.

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/mcp/Cargo.toml`
- Create: `crates/mcp/src/lib.rs`
- Create: `crates/mcp/src/tools.rs`

**Step 1: Add crate**

Add `crates/mcp` to workspace.

**Step 2: Add tool definitions**

Start with internal Rust functions before choosing final MCP transport:

```rust
analyze_source(source_id) -> diagnostics + statement analyses
analyze_query(query_text, schema_context) -> diagnostics + params + result type
hover(source_id, byte_offset) -> hover payload
schema_symbol(name) -> schema definition and references
```

**Step 3: Add tests**

Test the tool functions call workspace analysis and return structured data.

**Step 4: Verify**

```bash
cargo test -p surrealguard-mcp
```

Expected: passes.

**Step 5: Commit**

```bash
git add Cargo.toml crates/mcp
git commit -m "feat: add mcp analysis surface skeleton"
```

---

## Phase 6: Embedded host bridge spike

### Task 6.1: Add embedded source map model

**Objective:** Represent exact, approximate, and unmapped host-to-query mappings.

**Files:**
- Create or modify: `crates/workspace/src/embedded.rs`
- Add tests

**Step 1: Add model**

```rust
pub struct EmbeddedSourceMap {
    pub segments: Vec<SourceMapSegment>,
}

pub struct SourceMapSegment {
    pub query_range: ByteRange,
    pub host_range: ByteRange,
    pub fidelity: MappingFidelity,
}

pub enum MappingFidelity {
    Exact,
    Approximate,
    Unmapped,
}
```

**Step 2: Add mapping API**

```rust
pub fn map_query_range_to_host(&self, range: ByteRange) -> MappedRange;
```

Only return exact native spans when all covered segments are exact.

**Step 3: Add tests**

Test:

- single exact segment
- discontinuous segments
- approximate segment degrades
- unmapped segment degrades

**Step 4: Verify**

```bash
cargo test -p surrealguard-workspace embedded
```

Expected: passes.

**Step 5: Commit**

```bash
git add crates/workspace
git commit -m "feat: add embedded source maps"
```

### Task 6.2: Rust macro extraction spike

**Objective:** Prove compile-time extraction and diagnostics for one host adapter.

**Files:**
- Modify: `crates/macros/Cargo.toml`
- Modify: `crates/macros/src/lib.rs`
- Add UI tests if infrastructure exists, otherwise create a small compile-fail test crate

**Step 1: Keep scope small**

Initial `surql!` should support only string literals.

Do not support:

- dynamic expressions
- concat
- include_str
- typed wrappers beyond placeholder metadata

**Step 2: Extract query text**

Parse macro input and extract literal text.

**Step 3: Call workspace/analyzer**

Load config from project root if possible. If proc-macro environment makes this awkward, call a helper crate and document constraints.

**Step 4: Emit errors**

On diagnostics with error severity, emit compile error text containing:

- code number and name
- message
- exact SurrealQL snippet underline
- help if available

Native proc-macro span can be coarse for now.

**Step 5: Verify**

```bash
cargo test -p surrealguard-macros
cargo check --workspace
```

Expected: passes.

**Step 6: Commit**

```bash
git add crates/macros
git commit -m "feat: spike rust surql macro diagnostics"
```

### Task 6.3: TypeScript/JavaScript extraction design test

**Objective:** Prove tagged template mapping before building a full plugin.

**Files:**
- Create or modify: `crates/workspace/src/host/ts.rs`
- Add tests

**Step 1: Add pure extractor test helper**

Given a host source string containing:

```ts
const users = surql`SELECT * FROM user WHERE age > ${minAge}`;
```

Return:

- virtual query text: `SELECT * FROM user WHERE age > $host_0`
- one hole with host range
- exact source map segments for static template chunks

**Step 2: Add dynamic table test**

Input:

```ts
surql`SELECT * FROM ${table}`
```

Expected: virtual source plus a `dynamic.table_name` diagnostic or extraction marker.

**Step 3: Verify**

```bash
cargo test -p surrealguard-workspace ts_extraction
```

Expected: passes.

**Step 4: Commit**

```bash
git add crates/workspace
git commit -m "test: prove tagged template extraction model"
```

---

## Phase 7: Legacy quarantine and README update

### Task 7.1: Mark legacy crates and paths

**Objective:** Make old v1/v2-era code paths explicit without deleting them prematurely.

**Files:**
- Modify: `crates/core/README.md` or create it
- Modify: `crates/codegen/README.md` or create it
- Add module docs where useful

**Step 1: Add README notes**

State:

- `crates/core` is legacy v1 unless proven otherwise
- old codegen/watch flow is fallback/legacy
- new work should route through syntax/analyzer/workspace

**Step 2: Verify no behavior change**

```bash
cargo check --workspace
```

Expected: passes.

**Step 3: Commit**

```bash
git add crates/core crates/codegen
git commit -m "docs: mark legacy analysis paths"
```

### Task 7.2: Update README product framing

**Objective:** Align the public README with v3 direction after foundation exists.

**Files:**
- Modify: `README.md`

**Step 1: Update top-level pitch**

README should say SurrealGuard is a SurrealQL semantic intelligence engine for LSP, CLI/CI, MCP, and host-language integration.

**Step 2: Keep old codegen as fallback**

Mention generated files only as fallback, not primary product.

**Step 3: Add current status**

Be honest about in-progress v3 foundation.

**Step 4: Verify**

```bash
cargo check --workspace
```

Expected: passes.

**Step 5: Commit**

```bash
git add README.md
git commit -m "docs: align readme with v3 direction"
```

---

## Definition of done for v3 foundation

The v3 foundation is done when:

- `surrealguard-syntax` owns tree-sitter parsing and source/span primitives.
- Analyzer diagnostics carry stable codes, structured data, help, related info, and source ids.
- Lints support `allow`, `warn`, `deny`, and explicit suppressions.
- SurrealGuard has an owned type model distinct from `surrealdb::sql::Kind`.
- Generic built-in function signatures can be represented and tested.
- Workspace layer owns source registry, config, schema assembly, and shared analysis APIs.
- CLI `check`, LSP diagnostics, and MCP tools consume the same workspace/analyzer output.
- At least one embedded-query adapter spike proves source mapping and diagnostic presentation.
- Existing analyzer tests still pass.
- Legacy paths are clearly marked before removal.

## Deferred until after v3 foundation

- Full TypeScript language service plugin.
- Full Python type-checker plugin.
- Complete Rust typed query wrapper generation.
- Cross-language generated type packages.
- Live SurrealDB schema introspection.
- Rename/refactor of all legacy crates.
- Broad lint catalog beyond the initial small set.
