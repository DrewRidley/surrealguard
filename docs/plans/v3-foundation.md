# SurrealGuard Implementation Plan

## Goal

Build the ground-up SurrealGuard rewrite around stable syntax, diagnostics, types, workspace analysis, CLI, LSP, MCP, and host adapters.

`docs/DESIGN.md` is the source of truth. This plan tracks the next practical slices only.

## Ground rules

- Keep the maintained workspace small and explicit.
- Add tests for every public contract before or with implementation.
- Use `cargo test --workspace -- --nocapture` and `cargo check --workspace` as gates.
- Do not add generated-file or watch-mode behavior as a default product path.
- Do not add references to removed crate names in maintained code, tests, or docs.

## Current maintained workspace

- `crates/syntax`
- `crates/diagnostics`
- `crates/types`
- `crates/workspace`
- `crates/cli`
- `crates/lsp`

## Completed foundation

- Tree-sitter SurrealQL parse facade.
- Source IDs and source spans.
- Stable finding code families.
- Severity policy and suppression parsing.
- Owned type model and assignability rules.
- Generic function signature solving.
- Workspace source registry and syntax-analysis pipeline.
- CLI `check` routed through workspace analysis with stable JSON output.
- LSP diagnostics routed through workspace analysis.
- Removed replaced crates and codegen/watch commands from the maintained workspace.
- Schema indexing for `DEFINE TABLE` declarations, duplicate table diagnostics, and `DEFINE FIELD ... ON ... TYPE ...` declarations with simple type mapping.
- Basic query table-reference validation for `SELECT`, `CREATE`, `UPDATE`, and `DELETE`.
- Statement analysis records for parsed statements and parameter collection for `$param` references.
- Simple SELECT projection field validation against indexed `DEFINE FIELD` declarations.

## Completed slice: schema index

Objective: extract enough schema facts from `.surql` sources to support semantic diagnostics.

Implemented:

1. Added workspace-owned schema structs:
   - `SchemaIndex`
   - `TableDef`
   - `FieldDef`
2. Extracted `DEFINE TABLE <name>` declarations.
3. Preserved table and field definition spans.
4. Emitted duplicate table definition findings.
5. Returned the schema index in workspace analysis output.
6. Extracted `DEFINE FIELD <field> ON <table> TYPE <type>` declarations.
7. Preserved dotted field paths as structured field paths.
8. Mapped simple SurrealQL type names into `surrealguard-types`.
9. Emitted findings for fields on unknown tables.
10. Emitted explicit partial-analysis findings for unsupported field type syntax.

Verification:

```bash
cargo test --workspace -- --nocapture
cargo check --workspace
```

## Completed slice: basic query table references

Objective: validate table references in basic statements.

Implemented statement forms:

- `SELECT ... FROM <table>`
- `CREATE <table>`
- `UPDATE <table>`
- `DELETE <table>`

Tests:

- known tables pass
- unknown tables produce semantic findings
- spans point at the unknown table identifier
- source-specific diagnostics and workspace diagnostics receive the same finding
- syntax-error sources skip semantic table-reference validation to avoid noisy follow-on errors

## Completed slice: expression and result skeleton

Objective: introduce the shared semantic output shape before deeper inference.

Implemented:

1. Emit `StatementAnalysis` for parsed statements.
2. Preserve statement spans and stable statement kinds.
3. Collect `$param` references by name with source spans.
4. Keep `result_type: None` as the explicit result-shape placeholder.
5. Skip statement and parameter analysis for syntax-error sources to avoid noisy recovered-tree output.

Tests:

- each statement has a stable span
- parameters are collected and repeated references merge by name
- syntax-error sources do not emit statement or parameter analysis

## Completed slice: field-aware SELECT projection validation

Objective: validate simple projected fields against indexed schema fields.

Initial statement form:

- `SELECT <field>, <nested.field> FROM <table>`

Implemented:

1. Read simple projection fields from `SELECT <field>, ... FROM <table>` statements.
2. Validate projected field names against `DEFINE FIELD` declarations for the target table.
3. Skip wildcard projections and dynamic expressions until result-shape inference lands.
4. Preserve projection spans for CLI/LSP diagnostics.
5. Emit `E1004` findings for unknown projected fields.

Tests:

- known projected fields pass
- unknown projected fields produce semantic findings
- wildcard projections do not emit field diagnostics
- spans point at the unknown projected field identifier

## Slice 6: SELECT wildcard and result-shape scaffolding

Objective: move from validation-only SELECT analysis to statement result schemas.

Initial statement forms:

- `SELECT * FROM <table>`
- `SELECT <field>, <nested.field> FROM <table>`

Tasks:

1. Populate `StatementAnalysis.result_type` for simple SELECT statements.
2. Expand wildcard projections from indexed `DEFINE FIELD` declarations when available.
3. Infer projected object result shapes for simple field lists.
4. Keep schemaless, unknown, dynamic, and graph forms explicitly partial or unknown.

Tests:

- `SELECT * FROM person` returns an array/object shape with indexed fields when schema is known
- `SELECT name, profile.email FROM person` returns a projected object shape
- tables without field declarations do not claim precise field shapes
- graph traversal queries remain partial/unknown for now

## Slice 7: host adapter spike

Objective: prove embedded-query analysis with one host language.

Preferred first spike: Rust `surql!` macro checking against project schema.

Tasks:

1. Define embedded source mapping contract.
2. Extract literal macro body as an embedded SurrealQL source.
3. Run workspace analysis against it.
4. Map diagnostic spans back to the Rust source.

## Commit and verification discipline

Before each commit:

```bash
git status --short
git diff --cached --name-only
```

Each commit should leave:

```bash
cargo test --workspace -- --nocapture
cargo check --workspace
```

passing.
