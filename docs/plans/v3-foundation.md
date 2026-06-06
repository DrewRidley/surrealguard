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

## Slice 1: schema index

Objective: extract enough schema facts from `.surql` sources to support semantic diagnostics.

Tasks:

1. Add workspace-owned schema structs:
   - `SchemaIndex`
   - `TableDef`
   - `FieldDef`
   - `RelationDef`
2. Extract `DEFINE TABLE <name>` declarations.
3. Preserve definition spans.
4. Emit duplicate table definition findings.
5. Return the schema index in workspace analysis output.

Tests:

- table definitions are indexed by name
- definition spans point at the table name
- duplicate table declarations produce one stable semantic finding
- syntax findings and schema findings are returned together

Verification:

```bash
cargo test -p surrealguard-workspace -- --nocapture
cargo test --workspace -- --nocapture
cargo check --workspace
```

## Slice 2: schemafull field declarations

Objective: index field types from schema declarations.

Tasks:

1. Extract `DEFINE FIELD <field> ON <table> TYPE <type>`.
2. Support dotted field paths as structured field paths.
3. Map simple SurrealQL type names into `surrealguard-types`.
4. Emit findings for fields on unknown tables.
5. Preserve definition spans for field names and table names.

Tests:

- simple field declarations are indexed
- nested field paths are preserved
- unknown target table produces a semantic finding
- unsupported type syntax becomes an explicit partial-analysis finding

## Slice 3: basic query table references

Objective: validate table references in basic statements.

Initial statement forms:

- `SELECT ... FROM <table>`
- `CREATE <table>`
- `UPDATE <table>`
- `DELETE <table>`
- `RELATE <from> -> <relation> -> <to>`

Tests:

- known tables pass
- unknown tables produce semantic findings
- spans point at the unknown table identifier
- CLI and LSP surface the same finding model

## Slice 4: expression and result skeleton

Objective: introduce the shared semantic output shape before deeper inference.

Tasks:

1. Add `StatementAnalysis`.
2. Add `ResultShape` placeholders.
3. Add parameter collection for `$param` references.
4. Return partial-analysis markers for dynamic or unsupported structures.

Tests:

- each statement has a stable span
- parameters are collected with spans
- unsupported constructs do not panic and return explicit partial-analysis data

## Slice 5: host adapter spike

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
