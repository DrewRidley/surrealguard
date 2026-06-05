# SurrealGuard v3 Design

## Status

This document is the intended source of truth for the v3 redesign. Older docs in this repo are useful historical context, but they disagree in important places. When this document conflicts with older architecture, analyzer, LSP, or codegen docs, prefer this document.

## Product shape

SurrealGuard is a cross-language static analysis engine for SurrealQL.

It analyzes SurrealQL in:

- standalone `.surql` and `.surrealql` files
- schema and migration files
- embedded host-language queries, such as Rust macros and TypeScript tagged templates

It reports and exposes:

- syntax errors
- schema reference errors
- warnings and errors for type mismatches
- invalid graph traversals
- parameter type inference
- query result type inference
- expression and binding type hints
- hover information for tables, fields, functions, variables, graph edges, wildcards, parameters, and result shapes
- inlay hints for inferred non-trivial types
- go-to-definition and related definition links
- completions for tables, fields, variables, functions, keywords, and relation edges
- permission and access-shape warnings where the schema makes them statically visible

The primary product is not generated files or watch-mode codegen. The primary product is a reusable SurrealQL semantic intelligence engine. LSP, CLI/CI, MCP, and host-language adapters should all consume the same analysis output instead of reimplementing semantic rules.

Generated code may remain a fallback for languages or environments without native integration, but the first-class experience should feel native to the host language and editor.

Target first-class entry points:

1. `.surql` diagnostics and intelligence in editors such as Zed, Neovim, VS Code, and Helix through LSP.
2. CLI/CI checks through `surrealguard check` using the same diagnostics and inferred semantic model as LSP.
3. MCP access for LLM agents, exposing query analysis, schema lookup, hover/type information, and diagnostics as tool-callable structured data.
4. Rust `surql!` compile-time checking against the project schema.
5. TypeScript and JavaScript `surql` tagged-template checking through LSP/editor integration first, and later through a TypeScript language service plugin if deeper host type integration is needed.
6. Python `surql(...)` checking through LSP/editor integration first, and later through Pyright/Mypy plugin integration if useful.
7. Additional host adapters after the core source model proves stable.

The v3 minimum viable product is narrower than the full vision:

1. analyze physical `.surql` schema and query files with exact diagnostics
2. expose the same analysis through CLI and LSP
3. expose enough structured analysis for an MCP server to answer LLM-agent questions without scraping CLI text
4. preserve the current analyzer's tested semantic behavior
5. remove SurrealDB internal AST and type enums from public analyzer APIs
6. prove the embedded-source model with one host adapter spike before generalizing it

The MVP is complete when a project can run `surrealguard check`, receive precise source-linked diagnostics for schema/query mistakes, see the same diagnostics and type hints in an editor through LSP, and query the same semantic model from automation without generated files.

## Non-goals

SurrealGuard should not become a runtime query builder.

SurrealGuard should not depend on SurrealDB's internal Rust AST as the analyzer's semantic input.

SurrealGuard should not require generated type files for the normal editor or compile-time experience.

SurrealGuard should not hide uncertainty. If a query is too dynamic to analyze precisely, the analyzer should return an explicit partial-analysis result with clear diagnostics or notes.

## Parser strategy

SurrealGuard uses the maintained tree-sitter SurrealQL grammar as its parser foundation.

Tree-sitter is the right basis because it provides:

- byte spans for every concrete syntax node
- error recovery for partial and invalid queries
- incremental parsing for editor workflows
- language injection support for embedded queries
- a parser model that can be shared across standalone files, LSPs, and host-language adapters

SurrealDB's built-in Rust parser and AST should not be the analyzer input. The prior reason still stands: the parser may parse the language correctly, but its public AST is not designed as a stable, span-rich static-analysis interface. It also couples SurrealGuard to SurrealDB internals and release cadence.

SurrealDB's parser can still be useful as a test oracle:

- differential tests can compare whether tree-sitter accepts queries that SurrealDB accepts
- corpus tests can be built from SurrealDB's own parser tests and documentation examples
- compatibility tests can flag grammar drift

But production analysis starts from the tree-sitter CST.

Tree-sitter should be isolated behind a SurrealGuard syntax facade. Analyzer code should not scatter raw tree-sitter node-kind strings throughout semantic logic. The syntax layer should provide focused helpers such as:

- statement iteration
- identifier extraction
- expression child access
- source-span extraction
- error-node reporting
- compatibility utilities for grammar-version changes

This keeps grammar churn local. It also makes it possible to test the syntax facade directly before semantic analysis is involved.

### Tree-sitter integration contract

Tree-sitter should be treated as the concrete syntax source, not as the semantic AST. The integration should have three layers:

1. Raw parse tree: owned by `syntax`, useful for editor positions, error nodes, and grammar debugging.
2. Syntax facade: stable typed views over known SurrealQL constructs.
3. Semantic lowering: analyzer-owned statement and expression shapes that carry spans and preserve enough syntax detail for diagnostics.

Analyzer modules should operate on the semantic lowering or syntax facade, not on raw `Node::kind()` strings scattered across business logic.

A useful shape:

```rust
struct ParsedSource {
    source_id: SourceId,
    text: Arc<str>,
    tree: Tree,
    errors: Vec<SyntaxDiagnostic>,
}

struct SyntaxNode<'tree> {
    raw: Node<'tree>,
    source: SourceId,
}

struct LoweredStatement {
    kind: StatementKind,
    span: SourceSpan,
    syntax: SyntaxAnchor,
}

struct SyntaxAnchor {
    source: SourceId,
    byte_range: ByteRange,
    node_kind: &'static str,
}
```

`SyntaxAnchor` is intentionally lightweight. It lets diagnostics and hovers point back to syntax without making semantic data structures borrow the parse tree.

Tree-sitter error nodes should become `syntax.*` diagnostics with exact spans. The parser should keep producing a partial tree so the analyzer can still emit useful semantic diagnostics when possible.

Grammar compatibility should be tested independently:

- corpus parse tests for known-good SurrealQL examples
- recovery tests for incomplete editor input
- node-shape tests for constructs the analyzer depends on
- differential accept/reject tests against SurrealDB's parser where practical

## Core principle: precise analysis, graceful presentation

The analyzer should always preserve the most precise location and type information it knows.

Adapters may not always be able to present that information perfectly. That is acceptable. Do not weaken the analyzer model to fit the least capable adapter.

Examples:

- In `.surql` files, LSP can show exact squiggles.
- In TypeScript tagged templates, the adapter can usually map exact query spans back into the host file.
- In Rust proc macros, stable proc-macro APIs may only allow coarse native spans over the macro input or string literal. The macro should still render an exact SurrealQL snippet with the precise query-relative underline.

The diagnostic payload remains exact even when the host display is approximate.

## Source and span model

Internal positions are byte-based and source-relative.

The canonical diagnostic coordinate is:

```rust
struct SourceId(String);

struct ByteRange {
    start: u32,
    end: u32,
}

struct SourceSpan {
    source: SourceId,
    range: ByteRange,
}
```

The analyzer emits `SourceSpan`, not line/column positions.

Line and column are presentation concerns:

- CLI rendering maps bytes to line and column for snippets.
- LSP maps bytes to UTF-16 positions.
- Rust macro rendering maps query-relative bytes to a printed SurrealQL snippet and best-effort proc-macro span.
- TypeScript integration maps query-relative bytes through an embedded source map into the host file, then into TypeScript or LSP positions.

### Source kinds

A source can be a physical file or a virtual source extracted from a host file.

```rust
enum SourceKind {
    SurqlFile,
    EmbeddedQuery {
        host_source: SourceId,
        host_range: ByteRange,
        query_range: ByteRange,
        mapping: EmbeddedSourceMap,
    },
}
```

Physical `.surql` files have a direct one-to-one source span model.

Embedded queries have two coordinate spaces:

1. host-file coordinates
2. virtual SurrealQL query coordinates

The analyzer operates on virtual SurrealQL query coordinates. The adapter owns the mapping back to host-file coordinates.

Embedded source maps should be segment-based, not a single offset adjustment. Escapes, indentation trimming, raw strings, and template interpolation can make the mapping discontinuous.

```rust
struct EmbeddedSourceMap {
    segments: Vec<SourceMapSegment>,
}

struct SourceMapSegment {
    query_range: ByteRange,
    host_range: ByteRange,
    fidelity: MappingFidelity,
}

enum MappingFidelity {
    Exact,
    Approximate,
    Unmapped,
}
```

Adapters should only claim exact native spans when every byte in the diagnostic range maps through exact segments. Otherwise they should degrade to a coarser host span plus an exact query snippet.

## Embedded query extraction

Host adapters are responsible for finding SurrealQL query regions.

Examples:

- Rust: `surql!("SELECT * FROM user")`
- TypeScript: `` surql`SELECT * FROM user` ``
- Python: `surql("SELECT * FROM user")`

Extraction produces:

```rust
struct ExtractedQuery {
    source_id: SourceId,
    host_source_id: SourceId,
    host_range: ByteRange,
    query_text: String,
    source_map: EmbeddedSourceMap,
    holes: Vec<EmbeddedHole>,
}
```

The source map converts query-relative byte ranges back to host-source byte ranges when possible.

### Dynamic interpolation

Embedded templates can contain dynamic host expressions.

Example:

```ts
const users = await db.query(surql`
  SELECT * FROM user WHERE age > ${minAge}
`);
```

SurrealGuard should not pretend this is a static string. The adapter should lower interpolations into explicit holes.

A reasonable internal representation is:

```surql
SELECT * FROM user WHERE age > $host_0
```

With metadata:

```rust
struct EmbeddedHole {
    placeholder: String,       // "$host_0"
    host_range: ByteRange,     // range of `${minAge}` in the TypeScript file
    expected_type: Option<Type>,
}
```

The analyzer can infer `$host_0: int` from `age > $host_0`. The TypeScript adapter can then surface that as a host-language type expectation.

If interpolation appears in a syntactic position that cannot be represented as a parameter or analyzable placeholder, SurrealGuard should emit a partial-analysis diagnostic on the interpolation hole.

Example:

```ts
surql`SELECT * FROM ${table}`
```

This should not be silently accepted as fully analyzed. It should be reported as dynamic table name analysis is not supported unless the adapter has a safe way to enumerate the possible values.

## Diagnostics

Diagnostics are semantic facts produced by the analyzer.

The v3 rebuild should be organized around this classification model from the beginning. Diagnostics, warnings, lints, hints, partial-analysis notices, suppressions, and policy severity resolution are not output formatting concerns. They are the public contract that lets the same engine power editor UX, CI baselines, MCP responses, Rust macro errors, TypeScript transformer feedback, and future quick fixes without each adapter reinventing meaning.

A valid query can still produce lints. An invalid query should produce errors. A query that cannot be fully proven because of dynamic host-language input should produce dynamic/partial-analysis findings. These are separate categories even when they share one transport shape.

```text
syntax/schema/type/param/graph findings  -> correctness
permission/dynamic findings              -> safety and analysis confidence
lint findings                            -> configurable project policy
hint facts                               -> non-problem editor/agent context
```

```rust
struct Diagnostic {
    span: SourceSpan,
    severity: Severity,
    code: DiagnosticCode,
    message: String,
    help: Vec<Help>,
    related: Vec<RelatedInfo>,
    tags: Vec<DiagnosticTag>,
    data: DiagnosticData,
}

struct RelatedInfo {
    span: SourceSpan,
    message: String,
}

struct Help {
    message: String,
    replacement: Option<TextEdit>,
}
```

Related info must include a real `SourceSpan`, including source id. A byte span without source identity is not enough for cross-file work.

If the source identity for related info is unknown, omit the related info. Incorrect links are worse than missing links.

Diagnostic codes should be stable and documented. Message wording can improve over time, but adapter tests, editor integrations, and CI baselines need durable codes.

Suggested code families:

- `syntax.*` for parse and recovery diagnostics
- `schema.*` for unknown or invalid schema references
- `type.*` for type inference and compatibility diagnostics
- `param.*` for parameter inference and missing parameter information
- `graph.*` for graph traversal and relation-shape diagnostics
- `permission.*` for statically visible permission/access-shape warnings
- `dynamic.*` for partial-analysis or unsupported dynamic-query constructs

Every diagnostic should also carry enough structured information for adapters to render good output without parsing the human message. For example, an unknown-field diagnostic should include the field name and candidate table when available.

### Error codes, lints, and severity

SurrealGuard should use stable diagnostic codes. The code should be part of the public contract for CI baselines, editor tests, MCP output, suppressions, documentation links, and future quick fixes.

Use a Rust-style code plus category namespace:

```rust
enum DiagnosticCode {
    Syntax(SyntaxCode),      // S0001-S0999
    Schema(SchemaCode),      // E1000-E1999
    Type(TypeCode),          // E2000-E2999
    Param(ParamCode),        // E3000-E3999
    Graph(GraphCode),        // E4000-E4999
    Permission(PermissionCode), // W5000-W5999
    Dynamic(DynamicCode),    // W6000-W6999
    Lint(LintCode),          // L7000-L7999
}
```

Suggested numbering policy:

- `S0001-S0999`: syntax and parse recovery
- `E1000-E1999`: schema resolution errors
- `E2000-E2999`: type errors
- `E3000-E3999`: parameter inference and parameter binding errors
- `E4000-E4999`: graph/relation errors
- `W5000-W5999`: permission and access-shape warnings
- `W6000-W6999`: dynamic or partial-analysis warnings
- `L7000-L7999`: configurable lints

The letter is the default severity family, not a permanent severity. Config can promote warnings and lints to errors or demote some lints to allow.

Examples:

| Code | Name | Default | Meaning |
| --- | --- | --- | --- |
| `S0001` | `syntax.parse_error` | error | tree-sitter found an error node |
| `E1001` | `schema.unknown_table` | error | query references an unknown table |
| `E1002` | `schema.unknown_field` | error/warning by table mode | query references an unknown field |
| `E2001` | `type.mismatch` | error | expected one type, found another |
| `E2002` | `type.non_boolean_condition` | error | WHERE/ASSERT condition is not boolean |
| `E3001` | `param.unbound` | error | required parameter has no binding information |
| `E3002` | `param.conflicting_types` | error | one parameter is inferred with incompatible types |
| `E4001` | `graph.invalid_relation` | error | relation table does not connect the source/target shape |
| `W5001` | `permission.gated_field` | warning | selected field may be unavailable due to permissions |
| `W6001` | `dynamic.table_name` | warning | table name is dynamic and cannot be statically resolved |
| `L7001` | `lint.select_star` | allow | SELECT * is discouraged in configured contexts |

Diagnostics and lints share the same transport shape, but they differ conceptually:

- Errors: analysis found something invalid or unsafely incomplete.
- Warnings: analysis found something likely wrong or important but not always invalid.
- Hints: lightweight information that should not squiggle by default.
- Lints: configurable policy checks layered on top of correct analysis.

Lint configuration should support `allow`, `warn`, and `deny` levels, plus groups:

```toml
[lints]
select_star = "warn"
dynamic_query = "deny"
permission_gated_field = "warn"

[lints.groups]
style = "allow"
safety = "warn"
strict = "deny"
```

Suppression should exist, but it must be explicit and auditable. Prefer source comments in `.surql` files and host comments for embedded queries:

```surql
-- surrealguard: allow(lint.select_star) reason="admin export query"
SELECT * FROM user;
```

```ts
// surrealguard: allow(dynamic.table_name) reason="table is validated by enum before call"
surql`SELECT * FROM ${table}`;
```

Suppression rules:

- suppression should target a specific code or lint, not blanket-disable everything
- `deny` should override local `allow` unless config explicitly permits local overrides
- unused suppressions should be reported as a lint
- suppressions should require a reason in strict mode

Hints are not lower-quality diagnostics. They should be structured facts that adapters can choose to render as inlay hints, hover additions, code actions, or CLI notes.

### Diagnostic span rules

Use the smallest meaningful token or expression that is wrong.

| Problem | Span target |
| --- | --- |
| Unknown table | the table identifier |
| Unknown field | the field identifier |
| Assignment type mismatch | the right-hand value expression |
| Function argument type mismatch | the specific argument expression |
| Function argument count mismatch | the argument list |
| Missing required field in CREATE/INSERT | the target table identifier |
| Invalid graph relation | the invalid graph edge identifier |
| Invalid graph target | the invalid target table identifier |
| Non-boolean WHERE condition | the condition expression |
| Invalid cast | the cast target or source expression, whichever is more specific |
| Duplicate field assignment | the second field occurrence |

Avoid whole-statement spans unless the whole statement is truly the invalid unit.

### Host presentation tiers

Different integrations can present different span fidelity.

Tier 1: exact native span

- `.surql` LSP diagnostics
- host-language LSP diagnostics when the embedded source map is exact
- TypeScript tagged templates without problematic escaping or dynamic syntax holes

Tier 2: coarse native span plus exact SurrealQL snippet

- Rust proc macros on stable Rust when subspans inside string literals are not reliably available
- host APIs that only allow diagnostics on a full call expression or literal

Tier 3: whole-query diagnostic

- dynamically constructed strings
- unknown extraction patterns
- malformed host syntax where the query region cannot be mapped safely

The analyzer should still emit exact query-relative spans. Tiering is only about presentation.

## Type model

SurrealGuard should own its type model.

The analyzer should not re-export `surrealdb::sql::Kind` as its public type representation. SurrealDB's type enum is valuable as inspiration and may be used at compatibility edges, but SurrealGuard needs a stable analysis-oriented type model.

The type model needs first-class variants for at least:

- `Any`
- `Unknown { reason }`
- `Never`
- `Null` / `None`
- `Bool`
- `String`
- `Number` / `Int` / `Float` / `Decimal`
- `Datetime`
- `Duration`
- `Uuid`
- `Bytes`
- `Regex`
- `Range<T>`
- `Array<T>` and sized arrays when known
- `Set<T>`
- `Object` with named fields
- `Record<TableSet>` with table constraints
- `Geometry<GeometryKind>`
- `Option<T>`
- `Union<T...>`
- `Literal` values when useful for narrowing

It should represent analyzer uncertainty directly. For example, `Unknown` and `Any` should not mean the same thing:

- `Any`: SurrealQL/runtime allows any value.
- `Unknown`: SurrealGuard could not infer the type.

This distinction matters for diagnostics, strict mode, and host-language type emission.

Type compatibility should be explicit. Avoid burying SurrealQL coercion rules inside ad hoc match arms spread across statement analyzers.

The analyzer should centralize:

- assignability rules
- comparison compatibility
- arithmetic/operator result rules
- function signature matching
- record/table compatibility
- optional/null/none handling
- strict-mode behavior for `Unknown`

Strict mode should treat avoidable `Unknown` as a warning or error depending on context. Non-strict mode can allow more partial analysis, but it must still explain where precision was lost.

### Type representation and generics

SurrealGuard should model types structurally. Host-language type emission should be an adapter concern, not the core representation.

A useful core shape:

```rust
enum Type {
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

struct ObjectType {
    fields: IndexMap<FieldName, FieldType>,
    open: bool,
}

struct FieldType {
    ty: Type,
    optional: bool,
    readonly: bool,
    permission: Option<PermissionCondition>,
    span: Option<SourceSpan>,
}

struct GenericType {
    name: SmolStr,
    bounds: Vec<TypeBound>,
}
```

Generics are useful in three places:

1. Built-in function signatures, such as `array::len<T>(array<T>) -> int` or `array::first<T>(array<T>) -> option<T>`.
2. Host interpolation holes, where `$host_0` starts as a type variable and is constrained by query usage.
3. Adapter type emission, where a result shape may map to TypeScript generics, Rust structs, Python `TypedDict`, or JSON schema without changing analyzer logic.

Function signatures should not be hard-coded as one-off closures everywhere. Prefer a small signature language:

```rust
struct FunctionSig {
    name: FunctionName,
    generics: Vec<GenericParam>,
    params: Vec<ParamType>,
    returns: ReturnType,
}

enum ParamType {
    Exact(Type),
    Generic(GenericRef),
    OneOf(Vec<Type>),
    Variadic(Box<ParamType>),
}
```

Type inference should be constraint-based where that keeps the implementation simpler:

- collect constraints from expressions, comparisons, assignments, function calls, graph traversals, and interpolation holes
- solve constraints into concrete types or bounded unknowns
- emit `param.*` or `type.*` diagnostics for conflicts
- preserve unresolved type variables as `Unknown` with reasons, not as `Any`

This does not require a complex Hindley-Milner system. SurrealQL analysis can start with local constraints and explicit schema types. The important part is to avoid encoding every generic-looking operation as an ad hoc special case.

## Workspace model

The workspace layer owns files, source ids, schema assembly, and cross-file analysis.

Responsibilities:

- load `.surql`, `.surrealql`, schema, migration, and configured query files
- assign stable source ids
- track source text and line indexes
- extract schema definitions into a shared registry
- track definition locations with source id and byte range
- analyze documents against the workspace schema
- cache parse trees and analysis results where useful
- expose one API to CLI, LSP, Rust macro support, and future host adapters

The analyzer should not know about editor documents, watched folders, or LSP URIs. It should receive sources and contexts from the workspace layer.

Project configuration should be small and explicit. A likely config file is `surrealguard.toml`:

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

The workspace should also work with sensible defaults when no config exists:

- include `.surql` and `.surrealql` files under the project root
- ignore build output and dependency directories
- allow CLI flags to override config for CI experiments

Versioning matters. SurrealQL evolves with SurrealDB. The workspace should carry an intended SurrealDB compatibility version into syntax compatibility tests and semantic rules, even if v3 initially supports only one version well.

## Crate boundaries

Target crate layout:

```text
crates/
  syntax/        tree-sitter parser wrapper, CST helpers, source spans
  analyzer/      schema registry, type model, semantic analysis, diagnostics
  workspace/     project config, source registry, cross-file analysis, caches
  lsp/           LSP server and editor UX
  cli/           check/explain/init commands for CI and debugging
  mcp/           MCP server exposing analysis, schema, hover, and diagnostic tools
  rust/          Rust proc macro and compile-time adapter
  typescript/    TypeScript/JavaScript adapter support, if kept in this repo
  python/        Python extraction/type-checker adapter support, if kept in this repo
```

This is a target shape, not a requirement to rewrite everything at once.

Current code should be migrated carefully:

- preserve the tested behavior in `crates/analyzer`
- treat `crates/core` as legacy v1 unless proven otherwise
- treat `crates/codegen` and old watch-mode CLI as fallback or legacy paths
- move toward an owned type model before deep host-adapter work
- move source ids and cross-file definition locations into a workspace layer

## Public APIs

The analyzer API should make the main workflows obvious.

Possible shape:

```rust
struct AnalyzerInput<'a> {
    sources: &'a SourceRegistry,
    schema: &'a SchemaRegistry,
    source_id: SourceId,
}

struct AnalysisOutput {
    diagnostics: Vec<Diagnostic>,
    statements: Vec<StatementAnalysis>,
    inferred_params: Vec<ParamInference>,
    result_type: Option<Type>,
}
```

For a single query:

```rust
fn analyze_query(input: QueryInput) -> AnalysisOutput;
```

For a file:

```rust
fn analyze_source(input: SourceInput) -> AnalysisOutput;
```

For workspace-level analysis:

```rust
fn analyze_workspace(workspace: &Workspace) -> WorkspaceAnalysis;
```

The exact names can change, but the layering should stay intact:

- syntax parses
- analyzer reasons
- workspace connects files
- adapters present results

## Tooling surfaces

The same semantic model should be available through multiple surfaces:

1. LSP for editor interaction.
2. CLI for local checks, CI/CD, debugging, and machine-readable JSON output.
3. MCP for LLM agents that need to inspect schemas, analyze queries, retrieve inferred types, or ask for diagnostics without shelling out and parsing human text.
4. Host-language adapters for diagnostics and type information inside TypeScript, JavaScript, Python, Rust, and later other languages.

These surfaces should not have separate analyzers. They should share a stable `AnalysisOutput` and queryable semantic database.

Useful machine-facing operations:

```text
analyze_source(source_id) -> diagnostics + statement analyses
analyze_query(query_text, schema_context) -> diagnostics + params + result type
hover(source_id, byte_offset) -> hover payload
inlay_hints(source_id, range) -> inferred type hints
completion(source_id, byte_offset) -> completion items
definition(source_id, byte_offset) -> source locations
schema_symbol(name) -> schema definition and references
```

The CLI should expose this through flags such as `--format json`. The MCP server should expose the same operations as structured tools. The LSP should call the same APIs internally.

## LSP design

The LSP is a presentation and interaction layer.

It should be the first native product surface for `.surql` files and should work in editors that can speak LSP, including Zed. The same server should also be usable by editor extensions that route embedded host-language query ranges to SurrealGuard.

It should provide:

- diagnostics for syntax errors, schema errors, type mismatches, graph errors, and permission/access-shape warnings
- hover with inferred expression, parameter, result, table, field, function, variable, graph edge, and wildcard information
- inlay hints for inferred LET binding types and other non-trivial inferred types
- go to definition
- completions
- references and rename later
- code actions later

The LSP should not generate files on save by default.

The LSP should use the workspace layer for all cross-file analysis. It should not rebuild its own shadow schema model or rely on heuristic related-info filtering.

Hover and definition resolution should be AST-aware:

1. map LSP position to source byte offset
2. find tree-sitter node at offset
3. walk context to determine whether it is a table, field, variable, function, graph edge, wildcard, etc.
4. query analyzer/workspace semantic data
5. render concise output

## Host-language bridge

Older architecture notes correctly identified the bridge pattern: each host language adapter identifies SurrealQL regions in host code, extracts or maps those regions into virtual SurrealQL sources, then presents the same analyzer output in that host's native tooling.

The bridge is two-stage:

1. Universal stage: `.surql` analysis through syntax, analyzer, workspace, LSP, CLI, and MCP.
2. Host stage: TypeScript, JavaScript, Python, Rust, and other adapters reuse the universal stage, then map diagnostics, hovers, inferred types, completions, and definitions back into host files.

Initial host patterns:

| Language | Query shape | First bridge | Deeper native integration |
| --- | --- | --- | --- |
| TypeScript | `` surql`SELECT ... ${value}` `` | LSP/editor extraction with template source maps | TypeScript language service plugin for richer type inference |
| JavaScript | `` surql`SELECT ... ${value}` `` | same as TypeScript, but without static host types unless JSDoc or TS server data is available | JS-aware language service integration |
| Python | `surql("SELECT ...")` or project-configured wrapper calls | LSP/editor extraction with string source maps | Pyright or Mypy plugin for return/parameter types |
| Rust | `surql!("SELECT ...")` | proc macro compile-time checking | typed query wrapper and rust-analyzer support where possible |

Host adapters should not lower dynamic interpolation into raw string concatenation. Interpolation becomes explicit holes with source ranges and, when the host can provide it, host-language type information.

For TypeScript and JavaScript tagged templates, template literal text usually maps well enough for exact diagnostics inside static segments. Interpolations become `$host_N` placeholders. If the query uses an interpolation for a table name, field name, keyword, or other syntax-bearing position, the adapter should report a `dynamic.*` partial-analysis diagnostic unless it can safely enumerate possible values.

For Python strings, exact mapping depends on the string form. Plain strings, triple-quoted strings, raw strings, and f-strings need separate extraction rules. F-string expressions become holes like template interpolations. Escaped strings may produce approximate or unmapped segments.

For Rust macros, the macro can analyze at compile time, but stable proc-macro span presentation may be coarse. The macro should still emit exact SurrealQL snippets and structured diagnostic text derived from the same analyzer output.

## Rust adapter design

The Rust adapter should provide a real `surql!` macro.

Example:

```rust
let users = db.query(surql!("
    SELECT name, email FROM user WHERE age > $min_age
")).await?;
```

Compile-time flow:

1. macro extracts string literal text
2. macro or helper loads SurrealGuard project config
3. workspace loads schema files
4. analyzer checks the query
5. if diagnostics include errors, macro emits `compile_error!` with a rustc-style SurrealQL snippet
6. if clean, macro emits a typed query wrapper or typed metadata

Span reality:

- The analyzer emits exact query-relative spans.
- Stable proc macro presentation may only underline the string literal or macro invocation.
- The macro should render the exact inner SurrealQL span in the error text.
- If reliable subspans become available, the adapter can improve presentation without changing the analyzer.

## TypeScript adapter design

The TypeScript path should start with editor/LSP support for `surql` tagged templates.

Example:

```ts
const users = await db.query(surql`
  SELECT name, email FROM user WHERE age > ${minAge}
`);
```

Initial goals:

- diagnostics inside tagged templates
- parameter inference for interpolations
- hover result shape for query expressions
- table and field completions inside templates

A TypeScript transformer should not be the first assumption. Transformers complicate normal TypeScript projects. A language service plugin may be useful later if we need deeper type integration than LSP can provide.

## CLI design

The CLI should support CI and debugging.

Useful commands:

```text
surrealguard init
surrealguard check
surrealguard check --strict
surrealguard explain path/to/query.surql
surrealguard schema dump
surrealguard types dump --format typescript
```

`check` should be the CI path.

`explain` should print analyzer internals for a query:

- parsed source
- inferred params
- result type
- diagnostics
- related definitions

Type dumping or codegen can exist, but it should be explicit and secondary.

## Testing strategy

Tests should protect behavior during the redesign.

Required test layers:

1. Grammar corpus tests
   - SurrealDB docs examples
   - SurrealDB parser test examples where licensing and extraction are acceptable
   - project-specific tricky SurrealQL examples

2. Differential parser tests
   - tree-sitter parse result versus SurrealDB parser accept/reject behavior
   - used as a compatibility signal, not as production parsing

3. Analyzer unit tests
   - statement behavior
   - expression typing
   - function signatures and generic signature solving
   - graph traversal
   - permissions
   - result types
   - parameter inference
   - type compatibility rules

4. Diagnostic and lint tests
   - stable error codes
   - default severity
   - config-driven allow/warn/deny behavior
   - suppression comments
   - unused suppression detection
   - structured diagnostic data for adapters

5. Source mapping tests
   - `.surql` direct spans
   - TypeScript tagged template spans
   - escaped strings
   - interpolation holes
   - multiline templates

6. LSP tests
   - byte to UTF-16 conversion
   - diagnostics ranges
   - hover content
   - inlay hints for inferred types
   - go-to-definition locations

7. Machine-readable surface tests
   - CLI `--format json` schema remains stable
   - MCP tool outputs match analyzer outputs
   - hover/type/diagnostic payloads do not require parsing human messages

8. Host bridge source mapping tests
   - TypeScript and JavaScript tagged templates
   - Python plain strings, raw strings, triple-quoted strings, and f-strings
   - escaped strings
   - interpolation holes
   - multiline templates

9. Rust macro UI tests
   - compile-pass queries
   - compile-fail queries
   - rendered diagnostics include exact SurrealQL snippet and underline

## Migration plan

The redesign should be incremental.

Recommended sequence:

1. Establish this design doc as the source of truth.
2. Add a detailed foundation implementation plan under `docs/plans/`.
3. Introduce a syntax facade around tree-sitter without changing analyzer behavior.
4. Introduce source ids, source registry, and source spans without changing analyzer behavior.
5. Introduce stable diagnostic code enums, structured diagnostic payloads, and renderer-independent hints.
6. Add lint configuration and suppression parsing, initially with a small lint set.
7. Move tests out of `crates/analyzer/src/lib.rs` into focused test modules.
8. Introduce an owned type model behind compatibility conversions.
9. Add generic function signature support for built-ins and interpolation holes.
10. Split workspace concerns out of the LSP crate.
11. Rebuild CLI `check` on the workspace/analyzer path.
12. Add machine-readable analysis output for CLI and MCP.
13. Remove or quarantine legacy v1 crates after behavior is covered by v3 paths.
14. Build one embedded-query adapter spike, preferably Rust `surql!` if compile-time checking is the first product wedge.
15. Build TypeScript/JavaScript tagged-template editor support.
16. Build Python string/f-string editor support.

Do not start by deleting working analyzer behavior. The current analyzer test suite is the safety net.

Each migration step needs an acceptance gate:

- existing workspace tests still pass
- diagnostics keep or improve span precision
- no new public dependency on SurrealDB internal AST or analyzer-host internals
- CLI and LSP use the same analyzer output shape
- legacy paths are marked as legacy before removal

## Open questions

1. Should the maintained tree-sitter grammar live as an external dependency, a git submodule, or a vendored crate in this workspace?
2. What is the first host adapter after `.surql`: Rust or TypeScript?
3. How much type information should the Rust macro emit initially: diagnostics only, typed params, typed result, or full query wrapper?
4. Should dynamic table names ever be supported through enum-like host values, or should they remain explicitly unsupported?
5. What is the minimum useful TypeScript integration: LSP diagnostics only, hover result types, or actual TypeScript type inference?
6. When should old codegen be removed versus kept as a fallback CLI feature?
