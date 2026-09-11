# Full SurQL Semantics Implementation Plan

> **Superseded (2026-07-10).** Pre-rewrite plan; the semantics it describes were rebuilt on the typed AST — see `docs/DESIGN.md`. Kept for history.

> **For Hermes:** Use test-driven-development skill for every code slice. Do not start host adapters until this plan's core gates pass.

**Goal:** Model SurrealQL itself first: statement coverage, expression/value-kind inference, function signatures, block/variable environments, schema misuse diagnostics, graph semantics, mutation semantics, and response shapes should be stable in plain `.surql` before Rust proc macros, TypeScript transformers, or other host adapters are built.

**Architecture:** Keep tree-sitter SurrealQL CST as the syntax authority and SurrealDB public value/kind concepts as semantic leaf facts. Add an analyzer-owned semantic layer for facts SurrealDB public types do not provide directly: spans, field paths, statement facts, expression facts, function signatures, variable environments, response shapes, partial-analysis reasons, and graph relationship facts. CLI, LSP, MCP, and host adapters must consume this one core output rather than reimplementing rules.

**Tech Stack:** Rust workspace, `tree-sitter-surrealql`, `surrealdb_types::Kind`, `surrealql-analyzer-syntax`, `surrealql-analyzer-diagnostics`, `surrealql-analyzer-workspace`, local `surreal` smoke tests for behavior-dependent semantics, `cargo test --workspace -- --nocapture`.

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

Current implementation:

- added `ExpressionFact`, `ExpressionValueClass`, and `ExpressionDependencies` in `crates/workspace/src/expression.rs`
- expression facts carry `SourceSpan`, optional `surrealdb_types::Kind`, optional `ResponseShape`, value class, partial reasons, and dependency metadata
- exported expression fact types from `surrealql-analyzer-workspace`
- no analyzer pipeline behavior or diagnostics changed in this scaffolding slice

Tests:

- expression fact model preserves `Kind`, response shape, value class, and partial reasons
- expression fact model tracks dependency metadata without introducing a custom database type system
- existing SELECT/mutation response-shape tests remain the regression gate for unchanged behavior

Files:

- create `crates/workspace/src/expression.rs`
- modify `crates/workspace/src/lib.rs`

Verification:

```bash
cargo test -p surrealql-analyzer-workspace expression -- --nocapture
cargo test --workspace -- --nocapture
cargo check --workspace
```

### Slice B: literal, path, variable, object, array expression inference

Objective: infer expression facts for the basic building blocks used everywhere.

Current implementation:

- added `infer_expression_fact(node, parsed, row_table)` for tree-sitter expression nodes
- literals infer `Kind` and `ResponseShape::Value` for strings, bools, ints, and floats
- schema-backed row field paths infer field kind and value shape while recording field-path dependencies
- `$param`/`VariableName` records param dependency and remains unresolved until contextual inference binds it
- object literals infer closed `ResponseShape::Object` with child `FieldShape`s
- homogeneous arrays infer `Kind::Array(element, literal_len)` and array response shape
- mixed arrays remain conservative via `Kind::Any` element plus partial reason
- unwraps single-child `Fields`/`Predicate` grammar containers so callers can pass recovered CST nodes directly

Tests:

- literals map to `Kind`
- row field paths infer schema kind
- object literals infer object response shapes
- arrays infer element kind when homogeneous
- `$param` remains required unknown unless inferred from context

Deferred from this slice:

- nested object literal regression with child-object assertions
- variable environment for `LET $x = <expr>` across later statements
- assignability diagnostics that consume expression facts

### Slice C: assignability diagnostics for mutation payloads

Objective: validate value expressions assigned to schema fields.

Current implementation:

- emits `E2001` for statically known mutation value-kind mismatches
- covers `CREATE`/`UPDATE`/`UPSERT` field-level `SET` assignments
- covers object payloads in `CONTENT`, `MERGE`, and `REPLACE`
- covers object insert forms through existing object traversal
- covers tuple `INSERT INTO table (fields...) VALUES (values...)`
- covers nested object payload paths like `profile.email`
- covers `RELATE ... CONTENT` / `MERGE` edge payload values against relation table fields
- emits `E2002` when flat tuple `INSERT` value counts are not divisible by the field count
- checks tuple insert values across flattened multi-row `VALUES` forms by cycling schema fields over values
- uses `ExpressionFact` inference for literal/object/array/field/param facts
- skips dynamic params and unknown expressions instead of false-positive errors
- keeps unknown field diagnostics (`E1004`) separate from type mismatch diagnostics (`E2001`)

Tests:

- `CREATE person SET age = 'old'` reports `E2001`
- `UPDATE person MERGE { age: 'old' }` reports `E2001`
- `UPSERT person CONTENT { age: 'old' }` reports `E2001`
- `INSERT INTO person (age) VALUES ('old')` reports `E2001`
- `UPDATE person MERGE { profile: { email: 10 } }` reports nested `E2001`
- `RELATE person:one->likes->post:one CONTENT { created_at: 'yesterday' }` reports edge payload `E2001`
- `INSERT INTO person (age, name) VALUES (1)` reports arity `E2002`
- `INSERT INTO person (age, name) VALUES (1, 'Ada'), ('old', 'Grace')` checks both flattened rows
- dynamic params produce no type mismatch until contextual inference binds them

Deferred from this slice:

- richer assignability for unions, literal kinds, records, arrays, option/none/null, and object schemas
- exact tuple row boundary modeling beyond flattened arity modulo checks

### Slice D: mutation `RETURN <fields>` and `RETURN DIFF`

Objective: replace current mutation partials with verified shapes.

Current implementation:

- smoke-tested SurrealDB 3.0.5 locally against `UPDATE ... RETURN name, age`, `UPDATE ... RETURN DIFF`, and `DELETE ... RETURN name`
- infers `RETURN <fields>` as an array of closed projected objects using schema-backed field shapes, including nested field paths like `profile.email`
- infers `RETURN DIFF` as an array of patch arrays; each patch object has `op: string`, `path: string`, and `value: any`
- keeps `RETURN NONE` as an empty array shape

Deferred from this slice:

- field-return validation diagnostics for unknown mutation return fields
- exact `value` kind in `RETURN DIFF` patches
- expression/function returns inside mutation `RETURN`, beyond direct field projections

### Slice E: SELECT expression projections and aliases

Objective: model `SELECT 1 AS one`, function calls, binary expressions, and expression aliases without requiring field lookup.

Current implementation:

- `SELECT 1 AS one FROM person` returns projected field `one: Kind::Int`
- boolean/string/float literal projection aliases map to their literal kinds
- existing field aliases such as `SELECT name AS display FROM person` preserve the source field kind
- simple binary expression projections infer result kinds when both operands are statically known and compatible:
  - numeric `+`, `-`, `*`, `/` over `int`/`float` returns `int` unless either operand is `float`
  - string `+` over two strings returns `string`
- incompatible known binary operands emit `E2005`, for example `age + name` reports `operator `+` cannot combine `int` and `string``
- unaliased dynamic expressions use the expression text as a stable provisional field key

Deferred from this slice:

- richer expression operators beyond simple numeric/string binary compatibility
- function call signatures, arity diagnostics, and return kinds
- aggregate/group semantics such as `SELECT count() AS total FROM person GROUP ALL`

### Slice F: function signature table

Objective: implement built-in function arity, argument kind checking, and return inference.

Current implementation:

- verified SurrealDB 3.0.5 behavior locally for:
  - `string::len(name)` -> integer length
  - `array::len(tags)` -> integer length
  - `count()` -> integer aggregate/count value
  - `string::len()` runtime arity error wording
  - `string::len(tags)` runtime argument-kind error behavior
- parser CST shape is `FunctionCall(FunctionName, ArgumentList(...))`
- SELECT projection shape inference maps known function calls to `Kind::Int` return fields
- direct param arguments to known, correct-arity calls infer expected kinds:
  - `string::len($name)` -> `$name: string`
  - `array::len($tags)` -> `$tags: array<any>`
  - unknown functions and wrong-arity calls leave params unresolved to avoid false positives
- diagnostics now report:
  - `E2002` unknown function
  - `E2003` wrong arity
  - `E2004` wrong argument kind

Initial signature table:

- `count() -> int`
- `array::len(array) -> int`
- `string::len(string) -> int`

Deferred from this slice:

- function calls outside SELECT statement contexts
- richer signatures for `math::*`, `time::*`, `object::*`, `record::*`, etc.
- overloads, optional args, and SurrealDB-specific aggregate/group semantics beyond return kind

### Slice G: LET, RETURN, block, IF/ELSE, FOR semantics

Objective: make block statements analyzable before host adapters.

Current implementation:

- source-level LET facts collect literal/expression kind and shape for simple top-level declarations
- LET values can depend on earlier LET facts for simple variable and binary expressions, e.g. `LET $age = 42; LET $next = $age + 1; RETURN $next` resolves `$next` as `int`
- LET variables are excluded from query parameter inference only after their declaration has been encountered; a forward use such as `LET $y = $x + 1; LET $x = 1` still records `$x` as an external/dynamic param use
- repeated LET declarations use last-write-wins for later references and returns, matching SurrealDB 3.0.5 smoke behavior (`LET $x = 1; LET $x = 's'; RETURN $x` returns `'s'`)
- `RETURN $age` resolves to the LET variable response shape when the variable is statically known
- mutation assignability can use LET variable kinds, so `LET $age = 42; CREATE person SET age = $age` validates and `LET $age = 'old'; ... age = $age` reports `E2001`
- dependent LET variables participate in mutation assignability, so `LET $next = $age + 1; CREATE person SET age = $next` validates when prior facts establish `int`
- IF/ELSE statements infer RETURN branch response shapes, collapse identical branch shapes, and produce `ResponseShape::Union` for mixed branch returns; prior source-level LET facts are available inside branch RETURN expressions
- IF/ELSE conditions report `E2006` for statically-known non-bool kinds while allowing bool and unknown/dynamic conditions; SurrealDB 3.0.5 currently accepts truthy non-bool conditions, so this is a strict static type-compatibility diagnostic rather than a parser/runtime compatibility claim
- LET variables declared inside IF/ELSE blocks are branch-local for return-shape and parameter analysis: they can type later RETURN expressions in that block, but they do not leak to later top-level statements; verified SurrealDB 3.0.5 returns `null` for `RETURN $x` after `LET $x` only appeared inside an IF block

Deferred from this slice:

- complete lexical/block scopes remain deferred; current top-level LET facts plus IF block-local facts are conservative, not a general environment stack
- LET dependency support beyond simple prior variable references inside binary expressions
- full block-scoped LET/RETURN environment across arbitrary statement sequences
- FOR loop variable kind inference and block shape
- deeper branch-local nested/block variable behavior beyond the current IF block slice

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
- `surrealql-analyzer check --json` exposes enough structured facts for adapters
- docs list known static-analysis bypasses and runtime-only limits

Only after Slice K do we start host adapters.

## Adapter-readiness punchlist

The remaining core `.surql` work before host adapters is tracked in `docs/plans/2026-06-15-surql-adapter-readiness-punchlist.md`.

Use that punchlist as the active execution order after the current Slice G work: first unify the source-ordered statement environment, then migrate existing LET/RETURN/IF/param/mutation/select/function logic onto env snapshots, then expand expression, SELECT, mutation, graph, control-flow, schema object, JSON contract, and fixture coverage until the final adapter-readiness gate passes.

## Verification gates per slice

Every implementation slice must do:

```bash
cargo fmt
cargo fmt --check
git diff --check
cargo test -p surrealql-analyzer-workspace <targeted-filter> -- --nocapture
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
