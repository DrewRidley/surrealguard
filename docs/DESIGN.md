# SurrealGuard Design

## Status

This document is the source of truth for the ground-up rewrite. The maintained workspace is intentionally small and centered on stable contracts.

Current maintained direction:

- `surrealguard-syntax`: tree-sitter SurrealQL parsing, source IDs, spans, parse diagnostics.
- `surrealguard-diagnostics`: stable finding codes, severities, lint policy, suppression parsing.
- `surrealguard-workspace`: source registry, workspace config, schema facts, SELECT semantics, analysis orchestration.
- `surrealguard`: CLI surface.
- `surrealguard-lsp`: LSP diagnostics surface.

Legacy note: `crates/types` / `surrealguard-types` was removed during the v3 cleanup. The maintained design must not reintroduce a custom SurrealDB scalar/type hierarchy.

## Product shape

SurrealGuard is a cross-language static analysis engine for SurrealQL.

It analyzes SurrealQL in:

- standalone `.surql` and `.surrealql` files
- schema and migration files
- embedded host-language queries, such as Rust macros and TypeScript tagged templates

It exposes the same analysis model through:

1. CLI/CI via `surrealguard check`
2. LSP diagnostics and editor intelligence
3. MCP tools for agents
4. host-language adapters, starting with one embedded-query spike

The core product is the analysis engine, not generated files or watch-mode codegen. Generated files can exist later as an optional adapter, but they do not define the architecture.

## Non-goals

- No runtime query builder.
- No dependency on SurrealDB internal Rust AST as public analyzer APIs.
- No custom analyzer-owned SurrealDB scalar/type hierarchy. Schema leaf kinds must use upstream `surrealdb_types::Kind`, and syntax structure must come from tree-sitter CST nodes.
- No generated files required for the normal editor or compile-time experience.
- No compatibility promises for removed implementations.
- No hidden uncertainty. Partial analysis must be explicit.

## Parser strategy

SurrealGuard uses the maintained tree-sitter SurrealQL grammar as its parser foundation.

Tree-sitter provides:

- byte spans for every concrete syntax node
- error recovery for partial and invalid queries
- incremental parsing for editor workflows
- language injection support for embedded queries
- a shared parser model across standalone files, LSP, and host-language adapters

SurrealDB's parser is not the semantic input for this analyzer. SurrealGuard owns its source registry and span model, but query syntax is read from tree-sitter CST nodes. When semantic facts need SurrealDB value or kind concepts, use upstream SurrealDB public value/kind crates such as `surrealdb-types` instead of creating analyzer-local copies.

## Source and span model

Every analyzed input becomes a `Source` in the workspace registry.

A source has:

- stable `SourceId`
- display path or virtual name
- source text
- line index for byte to line/column mapping

All findings use `SourceSpan`, not raw file paths. Renderers decide how to display a span.

## Diagnostics contract

Findings are product contracts, not renderer details.

Code families:

- `Sxxxx`: syntax and parse findings
- `Exxxx`: semantic/type/schema errors
- `Lxxxx`: lints

A finding contains:

- stable code
- default severity
- effective severity after policy
- source span
- message
- optional structured notes later

Suppression syntax is explicit:

```surql
-- surrealguard: allow(L0001) reason
-- surrealguard: allow(unused-param) reason
```

Blanket suppressions are rejected.

## Semantic kind and response-shape model

SurrealGuard must not maintain a duplicate SurrealDB scalar/type hierarchy.

Rules:

- Parse structure from tree-sitter CST nodes.
- Store schema leaf kinds as upstream `surrealdb_types::Kind`.
- Use other actual SurrealDB public types where they fit: `Value`, `RecordId`, `Table`, `Object`, `Array`, `Set`, etc.
- Keep analyzer-owned structs only for analysis relationships that SurrealDB does not provide directly: source spans, response object fields, partial-analysis reasons, graph traversal facts, and host-adapter mappings.

Response schemas are analysis facts, not a new database type system. A response schema can say "array of objects with field `name` whose SurrealDB kind is `Kind::String`"; it should not introduce a competing `Type::String` enum.

Assignability and expression checks should be implemented in terms of SurrealDB `Kind` plus explicit analyzer rules. Unknown/dynamic/unsupported cases remain explicit partial-analysis facts, not permissive success.

## Workspace analysis

The workspace owns analysis orchestration.

Pipeline:

1. discover configured sources
2. register sources with stable IDs
3. parse each source through `surrealguard-syntax`
4. collect syntax findings
5. build schema index from `DEFINE` statements
6. analyze statements and expressions against schema and type environment
7. apply policy and suppressions
8. return structured `AnalysisOutput`

Current implementation covers source discovery, syntax diagnostics, table indexing, relation metadata for `DEFINE TABLE ... TYPE RELATION IN ... OUT ...`, field declaration indexing for simple schema kinds backed by `surrealdb_types::Kind`, broad static table-reference validation, statement analysis records, query parameter collection and simple field-comparison kind inference, SELECT IR extraction, SELECT projection field validation through that IR, row-context `WHERE` field validation, graph-local edge `WHERE` validation, row-preserving SELECT modifier facts, graph traversal diagnostics, relation-backed graph traversal response-shape inference, nested/object field-path modeling, mutation field diagnostics for `SET`/`UNSET`/object payloads/tuple insert columns, mutation `WHERE` diagnostics and param inference, RELATE endpoint validation, and conservative mutation response shapes for default/`RETURN BEFORE`/`RETURN AFTER`/`RETURN NONE`. `RETURN DIFF`, mutation `RETURN <fields>`, SELECT `GROUP`/`SPLIT`/`EXPLAIN`, block values, function calls, and broad expression semantics still produce explicit partial/unknown facts or are not yet fully modeled. The obsolete `surrealguard-types` crate has been deleted. Host-adapter inference is intentionally deferred until the core `.surql` engine has full statement/expression/type coverage.

Each parsed statement should eventually infer a response schema: the statement span and kind, input parameter requirements, result shape/type, and any partial-analysis limitations. Diagnostics should be emitted from that shared semantic model so CLI, LSP, MCP, and host adapters all explain the same facts rather than reimplementing rules per surface.

## CLI contract

`surrealguard check` is the CLI/CI entry point.

It must:

- load `surrealguard.toml` from the current directory or a parent
- discover configured `.surql` and `.surrealql` files
- run workspace analysis
- print human diagnostics by default
- print stable JSON with `--json`
- exit non-zero when any finding has effective severity `error`

## LSP contract

The LSP must consume the same workspace analysis output as CLI.

Current LSP scope:

- full text document sync
- workspace folder source scan
- diagnostics from the shared workspace pipeline

Future LSP features should only be added when the shared analysis output exposes the needed data:

- hover
- completion
- go-to-definition
- document symbols
- inlay hints
- signature help

## Embedded-source model

Embedded queries should not be special cases inside semantic analysis.

Host adapters should eventually produce sources with:

- host source ID
- embedded source text
- byte mapping from embedded query offsets back to host offsets
- host metadata needed for parameter/result integration

The semantic engine analyzes embedded sources through the same parser and workspace contracts. Host adapters are gated on core `.surql` readiness: no Rust macro, TypeScript transformer, or other adapter should own statement semantics, expression typing, function checking, graph validation, or response-shape inference. Adapters should only map host spans/parameter types into the shared engine once that engine has full plain-SurrealQL coverage.

## Next implementation slice

The typed AST layer is implemented (`docs/plans/2026-07-03-typed-ast-lowering.md`, including its completion-status table): all statement kinds lower to `surrealguard_syntax::ast` and type inference runs entirely on it, producing upstream `surrealdb_types::Kind` response types. The next work is the diagnostics phase — a comprehensive invariant list with finding codes, each incorporated into the analyzer that owns its statement, which also retires the frozen pre-AST validators in `semantic.rs`. Historical plans: `docs/plans/2026-06-12-full-surql-semantics.md`, `2026-06-06-select-semantics.md`, `2026-06-11-surql-statement-coverage.md`, `analyzer-module-rewrite.md` (all superseded in part).

Immediate focus:

1. introduce expression fact scaffolding without changing current behavior
2. infer literal/path/variable/object/array expression facts
3. validate assignability for mutation payloads and field assignments
4. model mutation `RETURN <fields>` and `RETURN DIFF`
5. expand SELECT expression projections, function signatures, block/LET/RETURN/IF/FOR semantics, graph traversal semantics, and remaining statement coverage

Host adapters may start only after the full core readiness gate passes.

Acceptance gates:

- no maintained code depends on `surrealguard-types` or `surrealguard_types`
- every parseable statement kind has stable analysis facts
- expression facts cover literals, paths, variables, objects, arrays, functions, subqueries, blocks, and dynamic/partial cases
- function calls have signature-based arity/argument/return analysis where statically known
- mutation, SELECT, graph traversal, block, and schema-object misuse diagnostics are emitted from the shared core
- response shapes are modeled or explicitly partial for every statement category
- host adapters do not implement independent SurrealQL semantic rules
- `cargo test --workspace -- --nocapture`
- `cargo check --workspace`
