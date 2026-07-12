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
3. MCP tools for agents (planned; not yet implemented)
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

The current implementation is the analyzer tree under `crates/workspace/src/analyzer/`: `analyzer::pipeline` walks each source in statement order, lowering every top-level statement to the typed AST and dispatching it to the analyzer that owns it, against the schema built from the statements before it (with `fn::` signatures hoisted). Each analyzer both infers its response `Kind` and enforces its contracts at the same site, per the diagnostic catalog (`docs/plans/2026-07-07-diagnostic-catalog.md`). Cross-statement contracts (transaction pairing, read-before-LET, `fn::` termination) live in the pipeline; parameter uses and typed constraints accumulate in the statement environment and are exported per source for host adapters. Schema extraction consumes the typed AST like everything else; tree-sitter appears only in `surrealguard-syntax` (and the embed crate's host grammars). Extraction mutates the schema index; the DDL contracts emit from the DEFINE/REMOVE/REBUILD/ALTER analyzers.

Each parsed statement should eventually infer a response schema: the statement span and kind, input parameter requirements, result shape/type, and any partial-analysis limitations. Diagnostics should be emitted from that shared semantic model so CLI, LSP, MCP, and host adapters all explain the same facts rather than reimplementing rules per surface.

## CLI contract

`surrealguard check` is the CLI/CI entry point.

It must:

- load `surrealguard.toml` from the current directory or a parent
- discover configured `.surql` and `.surrealql` files
- run workspace analysis
- print rustc-style human diagnostics by default (header, source excerpt with caret, `help:` suggestions, related locations); a clean run still prints surviving warnings, then the summary
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

The typed AST layer is implemented (`docs/plans/2026-07-03-typed-ast-lowering.md`, including its completion-status table): all statement kinds lower to `surrealguard_syntax::ast` and type inference runs entirely on it, producing upstream `surrealdb_types::Kind` response types. The diagnostics phase is implemented: the catalog's contracts emit from the analyzers that own their statements, and the pre-AST engine (`semantic.rs`, `select_ir.rs`, node-based expression inference) is deleted — `analyzer::pipeline` is the only walk. Next: host adapters over the parameter-constraint export, and the grammar-conformance worklist. Historical plans: `docs/plans/2026-06-12-full-surql-semantics.md`, `2026-06-06-select-semantics.md`, `2026-06-11-surql-statement-coverage.md`, `2026-06-15-surql-adapter-readiness-punchlist.md`, `2026-07-04-diagnostics-architecture.md`, `analyzer-module-rewrite.md` (all superseded in part).

Immediate focus:

1. host adapters over the parameter-constraint and response-kind exports, Rust proc-macro first
2. grammar-conformance worklist (`docs/grammar-conformance.md`) down to zero failures, then wire the harness into CI
3. TypeScript typegen: Kind-to-TS renderer and `surrealguard generate` over the embedded-query scan
4. remaining catalog machinery: 5010 event-trigger cycles, the 8xxx version registry, and the researched-but-unruled rows (4009, 4013)
5. richer `Finding.help`/`related` coverage (did-you-mean and declared-here attachments exist for tables, fields, `fn::` names, and relation shapes; extend site by site)

The core readiness gate below has passed; host adapters may start.

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
