# SurrealGuard Implementation Plan

## Goal

Build the ground-up SurrealGuard rewrite around stable tree-sitter syntax, diagnostics, SurrealDB-backed semantic kinds, workspace analysis, CLI, LSP, MCP, and host adapters.

`docs/DESIGN.md` is the source of truth. This plan tracks the next practical slices only.

## Ground rules

- Keep the maintained workspace small and explicit.
- Add tests for every public contract before or with implementation.
- Use `cargo test --workspace -- --nocapture` and `cargo check --workspace` as gates.
- Do not add generated-file or watch-mode behavior as a default product path.
- Do not add references that endorse removed crate names in maintained code, tests, or docs. Historical references must be explicitly marked as obsolete.

## Current maintained workspace

- `crates/syntax`
- `crates/diagnostics`
- `crates/workspace`
- `crates/cli`
- `crates/lsp`

Cleaned-up type model:

- `crates/types` / `surrealguard-types` has been removed from the maintained architecture. Use tree-sitter CST nodes for syntax and upstream `surrealdb_types::Kind` / other public SurrealDB types for semantic kind/value facts.

## Completed foundation

- Tree-sitter SurrealQL parse facade.
- Source IDs and source spans.
- Stable finding code families.
- Severity policy and suppression parsing.
- Removed obsolete owned type model and assignability rules in `surrealguard-types`; use `surrealdb_types::Kind` and response-shape facts instead.
- Historical generic function signature solving was removed with the obsolete type crate; reintroduce only if it can be expressed over upstream `Kind` and response-shape facts.
- Workspace source registry and syntax-analysis pipeline.
- CLI `check` routed through workspace analysis with stable JSON output.
- LSP diagnostics routed through workspace analysis.
- Removed replaced crates and codegen/watch commands from the maintained workspace.
- Schema indexing for `DEFINE TABLE` declarations, duplicate table diagnostics, and `DEFINE FIELD ... ON ... TYPE ...` declarations using `surrealdb_types::Kind` for known leaf kinds.
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
8. Maps simple SurrealQL type names into upstream `surrealdb_types::Kind`.
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
4. Current code keeps `response_shape: None` as the explicit placeholder until SELECT result-shape inference lands.
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

## Slice 6: completed type-system correction

Objective: correct the architecture before adding more SELECT semantics.

Implemented:

1. Deleted `crates/types` / `surrealguard-types`.
2. Added upstream `surrealdb-types` as a workspace dependency.
3. Store schema leaf kinds as `surrealdb_types::Kind`.
4. Replaced the old `StatementAnalysis` result-type placeholder with a response-shape contract that uses SurrealDB public kinds at the leaves.
5. Added workspace-owned `ResponseShape`, `FieldShape`, and `PartialReason` facts.

Comprehensive plan: `docs/plans/2026-06-06-select-semantics.md`.

Tests:

- schema field kinds map to `surrealdb_types::Kind`
- `SelectIr` extracts `AS`, `OMIT`, `FETCH`, `RETURN`, `ONLY`, row modifiers, and graph lookup nodes
- `SELECT * FROM person` returns an array/object response shape with indexed fields when schema is known
- `SELECT name, profile.email FROM person` returns a projected object response shape
- `SELECT VALUE name FROM person` unwraps the row object to an array of the field kind
- tables without field declarations do not claim precise field shapes
- graph traversal queries are represented in IR; simple relation-backed two-hop traversals infer target-table response shapes, while graph-local filters/selections remain partial

## Slice 7: SELECT IR and first response shapes

Objective: implement SELECT variants from `docs/plans/2026-06-06-select-semantics.md` now that the type-system correction is complete.

Completed:

1. tree-sitter SELECT IR extraction for fields, sources, aliases, `ONLY`, `OMIT`, `FETCH`, row modifiers, and graph lookup nodes
2. existing SELECT table-reference and projection validation routed through the IR
3. response-shape inference for schema-backed wildcard projections, named projections, aliases, and `SELECT VALUE <field>`
4. schema-backed `OMIT` shape subtraction
5. literal `LIMIT` array cardinality bounds
6. `FETCH` materialization flags on schema-backed fields
7. unknown `OMIT` and `FETCH` fields validated through the same `E1004` field diagnostic path
8. row-preserving SELECT modifier facts for `WHERE`, `ORDER`, `LIMIT`, `START`, `TIMEOUT`, and `PARALLEL`
9. explicit partial/unknown response shapes for `RETURN`, `GROUP`, `SPLIT`, and `EXPLAIN`
10. relation metadata indexing from `DEFINE TABLE ... TYPE RELATION IN ... OUT ...`
11. simple relation-backed graph traversal response-shape inference for two-hop traversals such as `person->likes->post`
12. row-context `WHERE` field validation against the resolved SELECT row table
13. parameter kind inference from SELECT predicates with field/parameter comparisons using `=`, `!=`, `<`, `<=`, `>`, and `>=`, including reversed operands and graph-local predicates
14. graph traversal diagnostics for unknown edge tables, unknown target tables, and relation endpoint mismatches, including parenthesized graph lookup selections
15. schemaless and dynamic-source SELECTs report explicit unknown/partial response shapes instead of claiming precision
16. graph-local `WHERE` validation for relation-edge context in parenthesized graph lookups and bracketed graph filters
17. nested/object field-path response shapes for schema-backed wildcard and projected fields, including parent-object validation and nested `OMIT`/`FETCH` handling

Remaining order:

1. broaden core `.surql` statement coverage before host adapters, tracked in `docs/plans/2026-06-11-surql-statement-coverage.md`
2. host adapter spike once the core response-shape/diagnostic facts are stable

## Slice 8: core SurQL statement coverage

Objective: make plain `.surql` analysis comprehensive enough that host adapters consume stable engine facts instead of inventing statement-specific behavior.

Completed:

1. stable `StatementAnalysis.kind` coverage for every parseable tree-sitter statement node currently exposed by the grammar
2. unknown static table diagnostics for additional non-SELECT statements: `UPSERT`, `INSERT INTO`, `LIVE SELECT`, `ALTER TABLE`, `REMOVE TABLE`, `REBUILD INDEX ... ON TABLE`, `SHOW CHANGES FOR TABLE`, and `INFO FOR TABLE/TB`
3. schema-backed mutation field diagnostics for statically named assignment fields and object keys in `CREATE`, `INSERT`, `UPDATE`, `UPSERT`, and `RELATE` data clauses
4. row-context `WHERE` field diagnostics and parameter-kind inference for `UPDATE`, `UPSERT`, and `DELETE`
5. `RELATE` source/edge/target endpoint validation from relation metadata
6. conservative non-SELECT response shapes for mutation statements: schema-backed row arrays for default/`RETURN BEFORE`/`RETURN AFTER`, empty arrays for `RETURN NONE`, and explicit partials for `RETURN DIFF`/field projections

Next:

1. model mutation `RETURN <fields>` projections and `RETURN DIFF` patch-array shapes when needed

## Slice 9: host adapter spike

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
