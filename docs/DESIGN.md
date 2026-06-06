# SurrealGuard Design

## Status

This document is the source of truth for the ground-up rewrite. The maintained workspace is intentionally small and centered on stable contracts.

Current crates:

- `surrealguard-syntax`: tree-sitter SurrealQL parsing, source IDs, spans, parse diagnostics.
- `surrealguard-diagnostics`: stable finding codes, severities, lint policy, suppression parsing.
- `surrealguard-types`: SurrealGuard-owned type model and assignability/signature contracts.
- `surrealguard-workspace`: source registry, workspace config, analysis orchestration.
- `surrealguard`: CLI surface.
- `surrealguard-lsp`: LSP diagnostics surface.

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
- No dependency on SurrealDB internal Rust AST or type enums as public analyzer APIs.
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

SurrealDB's parser is not the semantic input for this analyzer. SurrealGuard owns its source model and uses explicit conversion only where a future adapter requires it.

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

## Type model

SurrealGuard owns its type model.

The model covers:

- primitives: `none`, `null`, `bool`, `int`, `float`, `decimal`, `number`, `string`, `bytes`, `datetime`, `duration`, `uuid`
- structural types: arrays, sets, objects, options, unions
- SurrealDB-specific shapes: records, relations, geometry, futures
- analysis sentinels: `any`, `unknown`, `never`

Assignability is explicit:

- exact types assign
- integer/float/decimal widen to number
- optional accepts `none` and the inner type
- objects are structural and allow extra source fields
- record table sets are subset-compatible
- `unknown` is indeterminate, not success
- `any` is intentionally permissive

Function signatures use generics and solve against argument types.

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

Current implementation covers steps 1 through 4. Semantic work starts at schema indexing.

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

Host adapters should produce sources with:

- host source ID
- embedded source text
- byte mapping from embedded query offsets back to host offsets
- host metadata needed for parameter/result integration

The semantic engine analyzes embedded sources through the same parser and workspace contracts.

## Next implementation slice

The next slice is schema indexing in `surrealguard-workspace`:

1. introduce schema data structures owned by the workspace crate
2. extract `DEFINE TABLE` declarations from parsed SurrealQL
3. attach definition spans for later LSP definition links
4. surface duplicate table definitions as `Exxxx` findings
5. keep CLI and LSP unchanged except for consuming richer workspace output

Acceptance gates:

- focused tests for schema extraction and duplicate-table findings
- `cargo test --workspace -- --nocapture`
- `cargo check --workspace`
- no references in maintained code or tests to removed crate names
