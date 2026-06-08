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

Current implementation covers source discovery, syntax diagnostics, table indexing, relation metadata for `DEFINE TABLE ... TYPE RELATION IN ... OUT ...`, field declaration indexing for simple schema kinds backed by `surrealdb_types::Kind`, basic query table-reference validation, statement analysis records, query parameter collection, SELECT IR extraction, SELECT projection field validation through that IR, row-preserving SELECT modifier facts, and response-shape inference for schema-backed wildcard projections, named projections, aliases, `SELECT VALUE <field>`, `ONLY`, schema-backed `OMIT`, literal `LIMIT`, `FETCH` materialization flags, and simple two-hop relation-backed graph traversals such as `person->likes->post`. `RETURN`, `GROUP`, `SPLIT`, and `EXPLAIN` currently produce explicit partial response shapes. The obsolete `surrealguard-types` crate has been deleted. Deeper expression semantics, graph-local WHERE/selection semantics, richer relation traversal diagnostics, and host-adapter inference are still partial.

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

Host adapters should produce sources with:

- host source ID
- embedded source text
- byte mapping from embedded query offsets back to host offsets
- host metadata needed for parameter/result integration

The semantic engine analyzes embedded sources through the same parser and workspace contracts.

## Next implementation slice

SELECT IR and response-shape inference now cover schema-backed wildcard projections, named projections, aliases, `SELECT VALUE <field>`, `ONLY`, schema-backed `OMIT`, literal `LIMIT`, `FETCH` materialization flags, relation metadata, simple relation-backed graph traversal response shapes, row-preserving modifier facts, and explicit partials for shape-changing/unsupported SELECT clauses. The next slice is to move beyond response shape into row-context expression semantics:

1. validate row-context field references in `WHERE` and modifier expressions against the resolved row table
2. infer parameter facts from SELECT predicates and modifiers, starting with field/parameter comparisons
3. validate graph-local `WHERE` and graph selections against relation-edge context
4. expose richer diagnostics for graph traversal failures instead of only falling back to unresolved response shapes

The SELECT plan is `docs/plans/2026-06-06-select-semantics.md`.

Acceptance gates:

- no maintained code depends on `surrealguard-types` or `surrealguard_types`
- existing SELECT IR extraction tests keep covering fields, `AS`, `OMIT`, `FETCH`, `ONLY`, and graph lookups
- existing table-reference and projection diagnostics remain routed through SELECT IR
- focused tests for row-preserving modifiers, `OMIT` shape subtraction, literal `LIMIT`, `FETCH` materialization metadata, relation metadata indexing, and simple graph traversal response-shape inference
- dynamic/shape-changing modifier result shapes remain partial/unknown until modeled
- `cargo test --workspace -- --nocapture`
- `cargo check --workspace`
