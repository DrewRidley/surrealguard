# SurQL Statement Coverage Implementation Plan

> **For Hermes:** Use test-driven-development skill for each implementation slice.

**Goal:** Complete core `.surql` statement analysis before any host-language adapters, so Rust macros, TypeScript transformers, CLI, LSP, and MCP all consume the same statement, expression, diagnostic, parameter, and response-shape facts. This plan is now the narrower statement-coverage support plan; the broader adapter gate is `docs/plans/2026-06-12-full-surql-semantics.md`.

**Architecture:** Keep tree-sitter SurrealQL as the syntax authority. The workspace semantic layer records every parseable statement kind, validates statically knowable schema references in plain `.surql`, collects parameters everywhere, and infers response shapes only where semantics are understood. Host adapters come after this engine-level coverage is explicit and tested.

**Tech Stack:** Rust workspace, `tree-sitter-surrealql`, `surrealdb_types::Kind`, workspace semantic diagnostics, `cargo test --workspace -- --nocapture`.

---

## Current audit

The grammar currently exposes these top-level statement nodes:

- `AlterStatement`
- `BeginStatement`
- `BreakStatement`
- `CancelStatement`
- `CommitStatement`
- `ContinueStatement`
- `CreateStatement`
- `DefineStatement`
- `DeleteStatement`
- `ForStatement`
- `IfElseStatement`
- `InfoForStatement`
- `InsertStatement`
- `KillStatement`
- `LetStatement`
- `LiveSelectStatement`
- `OptionStatement`
- `RebuildStatement`
- `RelateStatement`
- `RemoveStatement`
- `ReturnStatement`
- `SelectStatement`
- `ShowStatement`
- `SleepStatement`
- `ThrowStatement`
- `UpdateStatement`
- `UpsertStatement`
- `UseStatement`

Before this plan, statement analysis only named `DEFINE`, `SELECT`, `CREATE`, `UPDATE`, `DELETE`, `INSERT`, `RELATE`, and `LET`. Basic unknown-table diagnostics covered `SELECT`, `CREATE`, `UPDATE`, and `DELETE`. SELECT has the richest response-shape and field-path model; other statements are mostly syntax/statement/parameter coverage plus partial table-reference checks.

## Coverage principles

1. Plain `.surql` is the foundation. A misuse detectable without host-language context should produce the same diagnostic in CLI/LSP/MCP before host adapters exist.
2. Every parseable statement node gets a stable `StatementAnalysis.kind`.
3. Every `$param` occurrence inside recognized statements is collected exactly once per name with all spans.
4. Schema/table/field misuse diagnostics are implemented for statically resolvable statement operands before response-shape inference.
5. Non-SELECT response shapes are added only after verified against SurrealDB behavior or documented as partial/unknown.
6. Dynamic table sources, function-produced tables, subquery targets, and runtime-only constructs should emit partial/unknown facts rather than fake precision.

## Statement coverage matrix

| Statement kind | Current target in core `.surql` | Response shape target |
| --- | --- | --- |
| `select` | already has table, projection, row/graph field, param-kind, graph relation, nested shape coverage | implemented for many SELECT forms; keep expanding from SELECT plan |
| `live_select` | validate source table and field/filter/fetch paths like SELECT where grammar permits | live stream/event shape later; partial until modeled |
| `create` | validate target table/record table, data-clause fields, params | full-row array shape for default/`RETURN BEFORE`/`RETURN AFTER`; empty array for `RETURN NONE`; projected-object array for `RETURN <fields>`; patch-array shape for `RETURN DIFF` |
| `insert` | validate `INTO` table and inserted object/column fields | full-row array shape for default/row-returning forms; empty array for `RETURN NONE`; projected-object array for `RETURN <fields>`; patch-array shape for `RETURN DIFF` |
| `update` | validate target table/record table, `SET`/`MERGE`/`REPLACE`/`PATCH`/`UNSET`, `WHERE`, `RETURN` | full-row array shape for default/`RETURN BEFORE`/`RETURN AFTER`; empty array for `RETURN NONE`; projected-object array for `RETURN <fields>`; patch-array shape for `RETURN DIFF` |
| `upsert` | same table/data/where coverage as UPDATE | full-row array shape for default/`RETURN BEFORE`/`RETURN AFTER`; empty array for `RETURN NONE`; projected-object array for `RETURN <fields>`; patch-array shape for `RETURN DIFF` |
| `delete` | validate target table/record table and `WHERE` fields | full-row array shape for default/`RETURN BEFORE`/`RETURN AFTER`; empty array for `RETURN NONE`; projected-object array for `RETURN <fields>`; patch-array shape for `RETURN DIFF` |
| `relate` | validate source/edge/target tables and relation endpoint compatibility; validate data fields on edge table | edge-row array shape for default/`RETURN BEFORE`/`RETURN AFTER`; empty array for `RETURN NONE`; projected-object array for `RETURN <fields>`; patch-array shape for `RETURN DIFF` |
| `define_table` | schema indexing, duplicate diagnostics, relation metadata | no runtime result shape initially |
| `define_field` | schema indexing, field-on-unknown-table diagnostics, supported `Kind` extraction | no runtime result shape initially |
| other `define_*` | statement kind and spans first; targeted semantic checks only as needed | no runtime result shape initially |
| `alter` | validate altered table exists | no runtime result shape initially |
| `remove` | validate table/field/index/event targets where table context is present | no runtime result shape initially |
| `rebuild` | validate `ON TABLE` exists | no runtime result shape initially |
| `show` | validate `SHOW CHANGES FOR TABLE` table exists | result is implementation-specific; partial initially |
| `info_for` | validate table target when `INFO FOR TABLE/TB <name>` is used | partial initially |
| `let` | collect params and later infer variable environment within source | statement value shape later |
| `return` | collect params; expression shape later | expression shape later |
| `throw` | collect params; value expression validation later | no normal result shape |
| `for` | collect params; validate body statements without double-counting top-level statement records | block/loop result later |
| `if_else` | collect params; validate branches without double-counting top-level statement records | union/partial branch shape later |
| transaction/control (`begin`, `cancel`, `commit`, `break`, `continue`, `sleep`, `use`, `option`, `kill`) | stable statement kind, spans, params where applicable | no response shape initially |

## Slice order

### Slice 1: complete statement analysis records

Objective: every parseable statement node in the grammar produces a stable statement kind.

Tests:

- one fixture containing every statement kind parses cleanly
- statement kinds match the matrix names in source order
- nested block statements do not become extra top-level records when the enclosing statement owns the node

Implementation:

- extend `statement_kind` in `crates/workspace/src/semantic.rs`
- keep `DEFINE` subkind extraction as-is for `define_table`, `define_field`, etc.

### Slice 2: non-SELECT table-reference diagnostics

Objective: catch obvious unknown-table misuse in plain `.surql` before host adapters.

Tests:

- unknown table diagnostics for `UPSERT`, `INSERT INTO`, `LIVE SELECT`, `ALTER TABLE`, `REMOVE TABLE`, `REBUILD INDEX ... ON TABLE`, `SHOW CHANGES FOR TABLE`, and `INFO FOR TABLE/TB`
- known table fixtures for the same statements produce no `E1003`
- spans point at the offending table identifier

Implementation:

- reuse `E1003` for unknown static table references
- add tree-sitter-backed helper(s) for table names following statement-specific keywords (`INTO`, `FROM`, `TABLE`, `ON TABLE`)
- do not diagnose dynamic values or subquery targets as unknown static tables

### Slice 3: mutation data-clause field validation

Objective: validate statically named fields in mutation statements against schema fields.

Statements:

- `CREATE ... CONTENT { ... }`
- `CREATE ... SET field = ...`
- `INSERT INTO table { field: ... }`
- `INSERT INTO table (field, ...) VALUES (...)`
- `UPDATE` / `UPSERT` `SET`, `MERGE`, `REPLACE`, `UNSET`
- `RELATE ... SET/CONTENT` against the edge table

Diagnostics:

- reuse `E1004` for unknown fields
- keep dynamic object spreads/variables partial rather than erroneous

Current implementation:

- validates statically named `FieldAssignment` nodes in `CREATE`, `UPDATE`, and `UPSERT` `SET`/`UNSET` clauses
- validates object keys in `CREATE CONTENT`, `INSERT INTO table { ... }`, bulk insert objects, `INSERT INTO table (field, ...) VALUES (...)`, `UPDATE`/`UPSERT` `MERGE`/`REPLACE`, and `RELATE ... CONTENT` against the relation edge table
- validates nested object keys as dotted field paths, using prefix existence for parent objects
- leaves dynamic object values/spreads partial rather than erroneous

### Slice 4: mutation `WHERE` parameter-kind and field validation

Objective: reuse SELECT predicate analysis for row-context predicates in `UPDATE`, `UPSERT`, and `DELETE`.

Current implementation:

- validates statically named `WHERE` fields in `UPDATE`, `UPSERT`, and `DELETE` against the target table schema
- reuses row-context comparison analysis to infer parameter kinds from mutation `WHERE` predicates
- supports reversed operand comparisons such as `$max_age >= age`

Diagnostics:

- reuses `E1004` for unknown row-context fields

### Slice 5: RELATE relation endpoint validation

Objective: apply relation metadata to `RELATE source->edge->target` statements.

Current implementation:

- validates `RELATE` source and target record tables against the relation edge table's `IN`/`OUT` metadata
- reports unknown or non-relation edge tables with graph diagnostics
- reports unknown target tables with graph diagnostics
- keeps edge data field validation on `RELATE ... CONTENT` against the relation table schema from the mutation data-clause slice

Diagnostics:

- reuses `E3001` for unknown/non-relation RELATE edge tables
- reuses `E3002` for unknown RELATE source/target tables
- reuses `E3003` for relation endpoint mismatches

### Slice 6: conservative non-SELECT response shapes

Objective: add response shapes for mutation statements only after verified behavior.

Current implementation:

- verified SurrealDB 3.0.5 behavior locally for default and `RETURN BEFORE|AFTER|DIFF|NONE|Fields` mutation forms
- infers schema-backed full-row array shapes for `CREATE`, `INSERT`, `UPDATE`, `UPSERT`, `DELETE`, and `RELATE` when the statement uses the default row-returning behavior or explicit `RETURN BEFORE`/`RETURN AFTER`
- represents `RETURN NONE` as an empty array shape (`max_len: Some(0)`)
- infers direct `RETURN <fields>` as a projected-object array using schema-backed field paths, including nested paths
- infers `RETURN DIFF` as an array of patch arrays with patch object fields `op: string`, `path: string`, and `value: any`
- keeps source-level `response_shape` set only when exactly one statement in that source has a response shape, avoiding ambiguous first-result behavior in multi-statement files

## Verification gates

Run before each commit:

```bash
cargo fmt --check
git diff --check
cargo test --workspace -- --nocapture
cargo check --workspace
```
