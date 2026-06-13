# Full SurQL Semantics Implementation Plan

> **For Hermes:** Use test-driven-development skill for every code slice. Do not start host adapters until this plan's core gates pass.

**Goal:** Model SurrealQL itself first: statement coverage, expression/value-kind inference, function signatures, block/variable environments, schema misuse diagnostics, graph semantics, mutation semantics, and response shapes should be stable in plain `.surql` before Rust proc macros, TypeScript transformers, or other host adapters are built.

**Architecture:** Keep tree-sitter SurrealQL CST as the syntax authority and SurrealDB public value/kind concepts as semantic leaf facts. Add an analyzer-owned semantic layer for facts SurrealDB public types do not provide directly: spans, field paths, statement facts, expression facts, function signatures, variable environments, response shapes, partial-analysis reasons, and graph relationship facts. CLI, LSP, MCP, and host adapters must consume this one core output rather than reimplementing rules.

**Tech Stack:** Rust workspace, `tree-sitter-surrealql`, `surrealdb_types::Kind`, `surrealguard-syntax`, `surrealguard-diagnostics`, `surrealguard-workspace`, local `surreal` smoke tests for behavior-dependent semantics, `cargo test --workspace -- --nocapture`.

---

## Non-negotiable gate

No host adapters until the core `.surql` engine has explicit, tested coverage for:

1. every parseable statement kind currently exposed by the grammar
2. expression kind inference for literals, variables, paths, records, arrays, objects, unary/binary ops, casts where grammar exposes them, subqueries, blocks, IF, FOR, and function calls
3. assignability checks for mutation payloads, field assignments, function arguments, LET/RETURN/block values, and schema field kinds
4. function argument and return inference for a useful built-in signature table, with unknown/dynamic functions represented as partial rather than silently accepted
5. response shapes for SELECT, mutation statements, RETURN, LET, IF/ELSE, FOR, and block-y statements where behavior is verified
6. graph traversal and relation semantics, including edge table compatibility, local edge filters/selections, target table resolution, graph response shapes, and graph-local param inference
7. stable diagnostics, spans, and machine-readable output that adapter authors can trust

This is a static analyzer, not an authorization/runtime verifier. Permissions, runtime cardinality, data-dependent function failures, dynamic table names, dynamic field names, string-built queries, and host-language control flow must remain explicit unknown/partial facts unless proven by the core input.

## Current baseline

Already implemented in the v3 branch:

- stable statement-kind records for all currently visible top-level statement nodes
- schema indexing for `DEFINE TABLE`, `DEFINE FIELD`, relation metadata, field kinds backed by `surrealdb_types::Kind`
- unknown table diagnostics for SELECT and many non-SELECT static table references
- SELECT IR for fields, aliases, `VALUE`, `AS`, `OMIT`, `FETCH`, `ONLY`, row-preserving modifiers, and graph lookups
- SELECT response shapes for schema-backed wildcard/projected fields, aliases, nested paths, `VALUE`, `ONLY`, `LIMIT`, simple two-hop graph traversals, `OMIT`, and `FETCH` materialization flags
- field-path diagnostics for SELECT projections, row `WHERE`, `OMIT`, `FETCH`, graph-local edge filters, mutation `SET`/`UNSET`, object payloads, tuple insert columns, and RELATE edge content
- parameter kind inference for simple field/param comparisons in SELECT WHERE, mutation WHERE, reversed comparisons, and graph-local predicates
- RELATE source/edge/target compatibility diagnostics from relation metadata
- conservative mutation response shapes for default/`RETURN BEFORE`/`RETURN AFTER`, `RETURN NONE`, and partials for `RETURN DIFF`/field projections

Main gaps:

- no first-class expression facts model yet
- no variable/block environment for LET, RETURN, IF/ELSE, FOR, nested blocks, or subqueries
- function calls are not checked against signatures
- broad expression assignability is not checked, so field/value kind mismatches mostly pass
- mutation `RETURN <fields>` and `RETURN DIFF` are still partial
- SELECT expression projections, aggregate/group semantics, split/group/explain, richer graph selection shapes, and record materialization through FETCH remain partial
- DEFINE FUNCTION, DEFINE PARAM, indexes, events, permissions, analyzer, access, and other schema objects are not semantically rich yet

## Semantic model additions

### 1. `ExpressionFact`

Create a workspace-owned expression fact model. It is not a duplicate DB type system.

Core fields:

- `span: SourceSpan`
- `kind: Option<surrealdb_types::Kind>`
- `shape: Option<ResponseShape>` for object/array/record layouts where `Kind` alone is insufficient
- `value_class`: literal, field path, variable, function call, subquery, object, array, graph path, block, unknown
- `partial: Vec<PartialReason>`
- `dependencies`: field paths, variable names, params, function name, subquery spans as needed

Use `Kind` for leaf facts. Use `ResponseShape` for returned object/array layout. Use partial reasons rather than pretending precision for dynamic expressions.

### 2. `TypeEnv` / scope model

Add a lexical-ish analysis environment for a single source:

- schema tables and fields from the whole workspace
- parameters collected from `$param`
- local variables from `LET $name = expr`
- block-local statement facts
- row context for SELECT/mutation/graph local filters
- edge context for graph-local predicates and selections
- function signatures from the built-in table and user-defined functions

Rules:

- Build schema globally before validation, as now.
- Analyze statements in source order for variables/functions/params.
- Nested blocks get child scopes. Parent variables are visible unless SurrealQL behavior says otherwise.
- Unknown variables emit diagnostics only when statically impossible, not for external `$param` usage.

### 3. `FunctionSignature`

Add a static signature table for built-ins plus user-defined function declarations.

Shape:

- name/family, e.g. `string::len`, `array::len`, `math::sum`, `type::thing`, `count`
- argument list with required/optional/rest markers
- accepted `Kind` sets or predicate helpers
- return kind/shape rule, which may depend on args
- purity/dynamic marker where useful for later linting, not for security claims

Diagnostics:

- `E2001`: expression kind mismatch / assignability mismatch
- `E2002`: unknown function
- `E2003`: wrong function arity
- `E2004`: invalid function argument kind
- `E2005`: unsupported/dynamic expression form that prevents a required static check
- keep `E100x` for schema/table/field misuse and `E300x` for graph misuse

Start with a deliberately small signature table and expand it in verified slices. Do not invent signatures from memory when behavior is unclear: smoke test `surreal` or mark partial.

## Coverage matrix

| Area | Required coverage before adapters | Notes |
| --- | --- | --- |
| Literals | none/null/bool/string/int/float/decimal/datetime/duration/uuid/bytes where CST exposes them | infer `Kind` and response value shape |
| Collections | arrays, sets if exposed, objects, nested objects | infer element/object shapes when homogeneous or schema-backed; partial for mixed until union rules land |
| Paths | row fields, nested paths, graph-local edge fields, fetched record paths | diagnose unknown fields, infer field kind, preserve spans |
| Variables | `$param`, `LET` vars, function args, loop vars | infer where possible; keep host-provided params unknown until host adapters map them |
| Binary/unary ops | comparisons, boolean ops, arithmetic, coalesce, regex/contains where grammar exposes them | validate operands and infer boolean/number/string results |
| Casts/type funcs | SurrealQL casts and `type::*` functions | verify behavior and encode signatures |
| Functions | built-in signatures, arity, argument kind checking, return inference | user-defined functions indexed later |
| SELECT projections | fields, aliases, `AS`, literals, function calls, expressions, `VALUE`, `*`, `OMIT`, `FETCH` | expression projections should no longer be unknown unless truly dynamic |
| SELECT modifiers | WHERE, ORDER, LIMIT, START, TIMEOUT, PARALLEL, GROUP, SPLIT, EXPLAIN | row-preserving facts plus explicit response-shape semantics where verified |
| Graph traversal | edge table compatibility, direction, local WHERE, edge selection, target table resolution, response shape | use relation metadata; dynamic graph pieces partial |
| Mutations | CREATE/INSERT/UPDATE/UPSERT/DELETE/RELATE target refs, payload fields, assignment fields, object payloads, field-value assignability, WHERE, RETURN forms | include both object `{}` and field-level `SET` definitions |
| Blocks | LET, RETURN, IF/ELSE, FOR, THROW, nested block values | infer block result or partial union as behavior is verified |
| Schema statements | DEFINE TABLE/FIELD/INDEX/EVENT/FUNCTION/PARAM/ACCESS/ANALYZER/etc. | index definitions and validate internal references where static |
| Control statements | BEGIN/CANCEL/COMMIT/USE/OPTION/KILL/SLEEP/BREAK/CONTINUE | statement facts and param/expression checks; no fake result precision |
| Diagnostics | stable code family, spans, notes, source-level and statement-level outputs | no adapter-specific semantic diagnostics |

## Slice plan

### Slice A: semantic fact scaffolding

Objective: introduce `ExpressionFact`, `StatementSemantics` helpers, and reusable assignability predicates without changing existing behavior.

Tests:

- expression fact model serializes with `Kind` leaves and partial reasons
- response shapes remain unchanged for existing SELECT/mutation tests
- no diagnostics change yet

Files:

- create `crates/workspace/src/expression.rs`
- modify `crates/workspace/src/lib.rs`
- modify `crates/workspace/src/semantic.rs` only to thread helpers where needed

Verification:

```bash
cargo test -p surrealguard-workspace expression -- --nocapture
cargo test --workspace -- --nocapture
cargo check --workspace
```

### Slice B: literal, path, variable, object, array expression inference

Objective: infer expression facts for the basic building blocks used everywhere.

Tests:

- literals map to `Kind`
- row field paths infer schema kind
- object literals infer object response shapes
- nested object literals preserve child fields
- arrays infer element kind when homogeneous and partial when mixed
- `$param` remains required unknown unless inferred from context
- `LET $x = <expr>` records variable kind for later statements in the same source

### Slice C: assignability diagnostics for mutation payloads

Objective: validate value expressions assigned to schema fields.

Tests:

- `CREATE person SET age = 'old'` reports `E2001`
- `UPDATE person MERGE { age: 'old' }` reports `E2001`
- `UPSERT person CONTENT { profile: { email: 10 } }` reports nested mismatch
- `INSERT INTO person (age) VALUES ('old')` reports mismatch
- valid `SET`, object payload, tuple insert, and RELATE edge payload forms pass
- dynamic vars produce param inference/partial, not false errors

### Slice D: mutation `RETURN <fields>` and `RETURN DIFF`

Objective: replace current mutation partials with verified shapes.

Tasks:

- smoke test SurrealDB 3.x for `CREATE/UPDATE/UPSERT/DELETE/RELATE RETURN <fields>` and `RETURN DIFF`
- infer projected object array shape for `RETURN <fields>` using the same projection engine as SELECT where possible
- infer `RETURN DIFF` patch-array shape, or a structured partial if patch payload shape is too dynamic
- keep `RETURN NONE` empty array behavior

### Slice E: SELECT expression projections and aliases

Objective: model `SELECT 1 AS one`, function calls, binary expressions, and expression aliases without requiring field lookup.

Tests:

- `SELECT 1 AS one FROM person` returns `one: Kind::Int`
- `SELECT name AS display FROM person` preserves alias kind
- `SELECT age + 1 AS next_age FROM person` infers numeric kind or partial if numeric rules are not precise yet
- `SELECT count() AS total FROM person GROUP ALL` follows aggregate/group semantics once verified
- expression without alias is either represented with a stable expression key or explicit partial

### Slice F: function signature table

Objective: implement built-in function arity, argument kind checking, and return inference.

Start signatures:

- `count()`
- `array::len(array)`
- `string::len(string)`
- `math::sum(array<number>)` or verified SurrealDB equivalent
- `type::table(string)` and `type::thing(table/string, id)` as dynamic-source helpers
- a small set of `time::*`, `string::*`, `array::*`, `object::*`, `record::*` only after behavior is verified

Tests:

- wrong arity reports `E2003`
- wrong arg kind reports `E2004`
- valid calls infer return kind
- unknown function reports `E2002` unless grammar/source marks it as dynamic/user-defined and unresolved
- function call params infer expected kind from signature

### Slice G: LET, RETURN, block, IF/ELSE, FOR semantics

Objective: make block statements analyzable before host adapters.

Tests:

- `LET $age = 42; CREATE person SET age = $age;` validates assignment through variable env
- `RETURN $age;` returns the variable shape
- `IF cond { RETURN expr } ELSE { RETURN other }` returns union/partial as appropriate
- `FOR $item IN [1,2] { RETURN $item; }` infers loop variable kind and block shape where verified
- branch-local variables do not leak unless verified behavior says they do

### Slice H: graph traversal completeness

Objective: cover graph edges beyond simple two-hop target shape.

Tests:

- edge-local WHERE validates edge fields and params
- edge-local SELECT/projection validates edge fields and infers shape
- parenthesized graph selection shape is modeled or explicit partial
- inbound and bidirectional traversals resolve target tables via relation metadata
- invalid graph endpoint/table combinations produce `E300x`
- `FETCH` of record fields can materialize target object shapes when record table metadata is known

### Slice I: SELECT advanced modifiers

Objective: model or explicitly diagnose shape-changing SELECT forms.

Tasks:

- verify SurrealDB behavior for `GROUP`, `GROUP ALL`, aggregate projections, `SPLIT`, `EXPLAIN`, SELECT `RETURN` if grammar accepts it
- infer verified shapes
- keep unverified/implementation-specific forms as partial with explicit reasons

### Slice J: schema object coverage

Objective: index and validate the rest of the DEFINE/REMOVE/ALTER ecosystem enough for static misuse diagnostics.

Targets:

- DEFINE INDEX: table and field refs, index name uniqueness per table where feasible
- DEFINE EVENT: table refs and WHEN/THEN expression field validation
- DEFINE FUNCTION: argument/env indexing, body RETURN shape, call signature
- DEFINE PARAM: param kind/default expression validation
- DEFINE ANALYZER/TOKENIZER/FILTER: statement facts and referenced analyzer objects where grammar exposes them
- DEFINE ACCESS/USER/SCOPE/TOKEN: statement facts and expression/param checks only where safe
- REMOVE variants: validate target object/table context when static
- ALTER variants: validate target table and field refs where static

### Slice K: result contract and readiness audit

Objective: prove adapter-readiness with plain `.surql` fixtures only.

Acceptance:

- a fixture file with all statement kinds has stable statement facts
- a fixture file with all main expression forms has stable expression facts/diagnostics
- a fixture file with mutation misuse has table/field/type/function diagnostics
- a fixture file with graph misuse has graph diagnostics
- a fixture file with dynamic/unknown constructs has explicit partial facts, not silence
- `surrealguard check --json` exposes enough structured facts for adapters
- docs list known static-analysis bypasses and runtime-only limits

Only after Slice K do we start host adapters.

## Verification gates per slice

Every implementation slice must do:

```bash
cargo fmt
cargo fmt --check
git diff --check
cargo test -p surrealguard-workspace <targeted-filter> -- --nocapture
cargo test --workspace -- --nocapture
cargo check --workspace
```

Behavior-dependent slices must also run a local SurrealDB smoke test and record the version, e.g.:

```bash
surreal version
surreal sql --endpoint http://127.0.0.1:<port> --namespace sg --database semantics --json --hide-welcome < /tmp/<fixture>.surql
```

## Static-analysis caveats

- Host-language string concatenation, conditionals, loops, and macro expansion are out of scope until adapters map embedded source fragments into core sources.
- Runtime permissions and auth are not guaranteed by this analyzer.
- Dynamic table names, field names, function names, graph edge names, and record IDs remain partial unless the source contains enough literal/static structure.
- SurrealDB version drift must be handled by smoke tests and docs. Do not encode unverified behavior as certain.
