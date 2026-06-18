# SurQL Adapter-Readiness Punchlist

> **For Hermes:** Use test-driven-development for every implementation slice. Do not start Rust, TypeScript, or other host adapters until the acceptance gate in this document passes.

**Goal:** Finish the plain `.surql` semantic engine so host adapters can pass query text and host param facts into one shared analyzer instead of reimplementing SurrealQL semantics.

**Architecture:** Build a source-ordered statement-sequence semantic pass around a reusable environment. Schema/catalog extraction stays global. Expression, statement, param, diagnostic, and response-shape facts should run against the environment snapshot at each source position. Unknown/dynamic constructs must be explicit partial facts, not silent gaps.

**Tech Stack:** Rust workspace, `tree-sitter-surrealql`, `surrealdb_types::Kind`, `surrealguard-syntax`, `surrealguard-workspace`, `surrealguard-diagnostics`, local SurrealDB 3.0.x smoke tests, `cargo test --workspace -- --nocapture`.

---

## Adapter-readiness definition

Host adapter spike is allowed only after all of these are true for plain `.surql`:

1. Every parseable statement kind has stable statement facts.
2. Every expression position that can be statically typed is analyzed through one expression fact path.
3. LET, RETURN, IF/ELSE, FOR, nested blocks, and subqueries use a source-ordered environment.
4. Mutations, SELECT, graph traversal, function calls, and control-flow statements consume the same environment snapshots.
5. Response shapes are statement-level, stable, and explicit about unknown/partial constructs.
6. Diagnostics have stable codes, spans, and machine-readable JSON output.
7. A readiness fixture set covers valid, invalid, and dynamic/partial SurQL without host-language context.

Non-goal: prove runtime authorization, permissions, data-dependent cardinality, dynamic string-built queries, or host-language control flow.

---

## Current completed foundation

Already implemented and should be preserved while refactoring:

- statement-kind coverage for visible top-level grammar nodes;
- schema indexing for tables, fields, relation metadata, and duplicate definitions;
- unknown table/field diagnostics across SELECT, mutations, RELATE, and graph-local contexts;
- SELECT IR for fields, aliases, VALUE, AS, OMIT, FETCH, ONLY, row-preserving modifiers, and graph lookup nodes;
- SELECT response shapes for schema-backed wildcard/projection, aliases, nested paths, VALUE, ONLY, LIMIT max length, Omit, Fetch materialization flags, and simple graph traversal;
- mutation assignability for SET, CONTENT, MERGE, REPLACE, object INSERT, tuple INSERT, nested object fields, and RELATE edge payloads;
- mutation response shapes for default row-returning forms, RETURN BEFORE/AFTER, RETURN NONE, RETURN DIFF, and RETURN field projections;
- RELATE endpoint compatibility against relation metadata;
- expression facts for basic literals, field paths, params, objects, arrays, simple binary expressions, and a tiny function table;
- function diagnostics/return inference for `count()`, `string::len(string)`, and `array::len(array)`;
- source-order top-level LET facts, shadowing, dependent LETs, param distinction for forward uses;
- IF/ELSE condition diagnostics, branch RETURN shape merging, and IF block-local LET isolation.

---

## Phase 1: unify the semantic spine

Objective: remove the current one-off pass shape and create the shared pipeline every later feature uses.

### 1.1 Introduce `StatementEnv`

Status: initial scaffolding implemented in `crates/workspace/src/statement_env.rs`.

Add an analyzer-owned environment carrying at least:

- visible LET variables and their `ExpressionFact`/shape/kind;
- inferred external params and spans;
- schema/catalog reference;
- function signature table;
- optional row context;
- optional edge context;
- partial-analysis facts.

Acceptance:

- existing tests pass with no semantic behavior change;
- environment cloning/forking is available for nested blocks;
- no host-adapter concepts appear in the env.

### 1.2 Add `analyze_statement_sequence`

Status: initial implementation routes source-level statement facts, ordered param collection, top-level LET effects, and block-local param/LET isolation through `analyze_statement_sequence`. Response-shape and downstream diagnostics migration continues in later Phase 1 slices.

Analyze statement nodes in source order:

- emit statement facts;
- run statement-local diagnostics;
- infer response shape when known;
- update env for LET and later DEFINE PARAM/FUNCTION slices;
- return final env only for scopes whose bindings leak.

Acceptance fixture:

```surql
LET $age = 42;
CREATE person SET age = $age;
IF $age > 18 { LET $label = 'adult'; RETURN $label; } ELSE { RETURN 'minor'; };
RETURN $label;
```

Expected:

- mutation assignability sees `$age: int`;
- IF condition is analyzed against `$age`;
- branch RETURN sees branch-local `$label`;
- final top-level RETURN does not resolve `$label` as a LET var;
- `$label` outside the branch is external/dynamic or unresolved, not silently resolved.

### 1.3 Move existing LET/RETURN/IF/param logic onto the sequence pass

Status: partially complete. Source-level statement facts, param collection, top-level LET effects, block-local LET isolation, RETURN response shapes, IF condition diagnostics, and IF branch RETURN merging now run through env-backed source-order walks. Other downstream validators still need env-snapshot migration.

Migrate current behavior without broad feature expansion:

- ordered param collection;
- top-level LET shadowing;
- dependent LET inference;
- RETURN shape inference;
- IF condition diagnostics;
- IF branch RETURN merging;
- IF block-local LET isolation.

Acceptance:

- all existing LET/IF/param tests pass;
- no duplicated source-order LET logic remains outside the sequence pass except compatibility wrappers.

### 1.4 Make downstream validators consume env snapshots

Status: partially complete. Mutation assignability, SELECT function-call argument diagnostics, and binary-expression diagnostics in LET/RETURN/block expression contexts now consume env-scoped LET facts, including branch-local LETs inside blocks. SELECT expression projections, WHERE param kind inference, graph-local predicates, function diagnostics outside SELECT/LET/RETURN expression walks, and mutation RETURN expressions/fields still need migration.

Route existing validators through the statement env where expressions are involved:

- mutation assignability;
- function calls;
- SELECT expression projections;
- WHERE param kind inference;
- graph-local predicates;
- mutation RETURN expressions/fields.

Acceptance:

- a LET variable can affect every statically typed expression position where an equivalent literal would be accepted;
- forward/dynamic params still avoid false-positive type mismatches;
- existing diagnostic codes and spans remain stable unless intentionally updated.

---

## Phase 2: expression coverage completion

Objective: all statically knowable expression forms in common SurQL positions flow through `ExpressionFact`.

### 2.1 Literal and scalar kinds

Cover CST-exposed scalar literals beyond the current core:

- `none` / `null` distinction where `Kind` exposes it;
- decimal;
- datetime;
- duration;
- uuid;
- bytes;
- record IDs / things.

Acceptance:

- value kind and response shape are inferred where static;
- assignability behavior is verified against SurrealDB for null/option-like field kinds before strict diagnostics.

### 2.2 Unary, boolean, comparison, and coalesce operators

Add operator facts for:

- boolean `AND`/`OR`/`!` or grammar equivalents;
- comparisons returning bool;
- arithmetic unary operators;
- coalesce/default operators if exposed by the grammar;
- regex/contains operators where grammar exposes them.

Acceptance:

- conditions such as `IF age > 18` infer bool;
- WHERE expressions get operand diagnostics and param-kind inference;
- unsupported/dynamic operator forms emit partial facts rather than fake precision.

### 2.3 Casts and type-related functions

Cover SurrealQL casts and verified `type::*` signatures.

Acceptance:

- behavior-dependent casts are smoke-tested with local SurrealDB;
- invalid static casts get diagnostics only when SurrealDB semantics are clear;
- dynamic casts remain partial.

### 2.4 Subqueries and block expressions

Model expression positions that contain nested queries/blocks.

Acceptance:

- nested statement sequences use child envs;
- result shape is derived when a single static result is clear;
- multi-result or data-dependent subqueries are explicit partials.

---

## Phase 3: functions and user-defined callables

Objective: function analysis is useful enough for real queries and not SELECT-only.

### 3.1 Expand built-in signature table by verified families

Add signatures in small verified groups:

- `math::*` numeric functions;
- `string::*` common string transforms;
- `array::*` common collection functions;
- `object::*` common object functions;
- `time::*` date/time functions;
- `record::*` / `type::*` where statically useful.

Acceptance per group:

- local SurrealDB smoke tests record behavior;
- arity diagnostics;
- argument-kind diagnostics;
- return kind/shape inference;
- param-kind inference for direct param args.

### 3.2 Apply function diagnostics outside SELECT

Function calls should be checked anywhere expression facts are used:

- LET values;
- RETURN expressions;
- mutation assignments and payloads;
- IF/FOR conditions;
- WHERE predicates;
- DEFINE defaults/bodies where supported.

Acceptance:

- one function checker path, no SELECT-only duplicate logic.

### 3.3 Index `DEFINE FUNCTION`

Support static user-defined function signatures.

Acceptance:

- function name, params, and return/body facts are indexed;
- calls to known user functions get arity and argument checks;
- body RETURN shape is inferred when static;
- recursive/dynamic functions become partial, not crashes or bogus certainty.

---

## Phase 4: SELECT completion

Objective: SELECT is adapter-grade for common production queries.

### 4.1 Predicate and modifier expressions

Analyze expressions in:

- WHERE;
- ORDER BY;
- LIMIT;
- START;
- TIMEOUT;
- PARALLEL where expression-like;
- WITH/EXPLAIN forms if grammar exposes static operands.

Acceptance:

- field refs, params, functions, LET vars, and operators are analyzed consistently;
- invalid static field/operator/type use gets stable diagnostics.

### 4.2 Aggregates and grouping

Verify and model:

- `GROUP BY` / `GROUP ALL`;
- aggregate projections such as count/sum/avg/min/max;
- grouped row shape rules;
- non-grouped field behavior.

Acceptance:

- verified response shapes for common aggregate queries;
- unmodeled aggregate behavior is explicit partial.

### 4.3 SPLIT, EXPLAIN, and other shape-changing modifiers

Smoke-test and model or explicitly partialize:

- SPLIT;
- EXPLAIN;
- any SELECT result modifiers accepted by grammar but not currently shaped.

Acceptance:

- no accepted SELECT modifier silently disappears from analysis.

---

## Phase 5: mutation completion

Objective: all mutation expression positions and result forms use the same expression/env system.

### 5.1 Mutation WHERE and assignment expressions through env

Ensure LET vars, functions, operators, casts, and params are checked in:

- SET values;
- CONTENT/MERGE/REPLACE object values;
- PATCH where static;
- WHERE predicates;
- tuple INSERT values.

Acceptance:

- every static mismatch that would be caught with a literal is also caught through a LET/function/expression chain.

### 5.2 Mutation RETURN expressions

Go beyond direct field projections:

- `RETURN <field>` already exists;
- support expression/function aliases if grammar accepts them;
- validate unknown fields in mutation RETURN;
- keep DIFF value kind conservative unless statically known.

Acceptance:

- mutation RETURN shape logic uses expression facts and schema row context.

---

## Phase 6: graph traversal completion

Objective: graph traversal is semantically useful, not just syntactically recognized.

### 6.1 Direction and endpoint resolution

Cover:

- outbound;
- inbound;
- bidirectional;
- multi-hop chains;
- parenthesized graph selections.

Acceptance:

- target table resolution follows relation metadata when static;
- unknown/dynamic edge/table pieces become partial facts;
- endpoint mismatches emit graph diagnostics.

### 6.2 Edge-local filters and selections

Analyze edge-local:

- WHERE;
- SELECT/projection;
- params;
- functions/operators;
- response shape.

Acceptance:

- edge context and target row context are distinct and correct.

### 6.3 FETCH and record materialization

Strengthen record/object materialization:

- fetched record fields become materialized object shapes when table metadata is known;
- nested fetches preserve shape/partial metadata;
- dynamic record table names remain partial.

---

## Phase 7: block and control-flow completion

Objective: control-flow constructs are explicit and predictable.

### 7.1 FOR loops

Verify SurrealDB behavior and model:

- loop variable kind from iterable expression;
- body statement sequence with loop variable in env;
- RETURN/THROW/BREAK/CONTINUE behavior inside body;
- loop result shape or partial result.

Acceptance:

- no FOR body diagnostics are skipped;
- loop-local variables do not leak unless verified.

### 7.2 THROW, BREAK, CONTINUE, SLEEP, KILL, USE, OPTION, transactions

For each statement:

- stable statement fact;
- param/expression analysis where applicable;
- control-flow/result effect if statically meaningful;
- explicit no-shape/partial where not meaningful.

Acceptance:

- control statements are not just named, their statically checkable operands are analyzed.

### 7.3 Nested IF/ELSE and exhaustiveness

Generalize branch analysis:

- nested branches;
- ELSE IF chains;
- missing ELSE partial fallthrough;
- return shape unioning with fallthrough awareness.

Acceptance:

- branch-local scoping remains correct;
- non-exhaustive branches are explicit partials.

---

## Phase 8: schema object completion

Objective: DEFINE/REMOVE/ALTER statements provide useful static checks and facts.

### 8.1 DEFINE PARAM

Index static params:

- declared kind/default;
- default expression validation;
- later `$param` references use declared kind.

Acceptance:

- DEFINE PARAM reduces unknown external param facts where applicable.

### 8.2 DEFINE INDEX / EVENT / ANALYZER

Validate static references:

- index table and fields;
- event table and WHEN/THEN field refs/expressions;
- analyzer/tokenizer/filter object names where grammar exposes them.

Acceptance:

- schema misuse gets stable diagnostics;
- dynamic bodies remain partial.

### 8.3 DEFINE ACCESS / USER / SCOPE / TOKEN / permissions

Record statement facts and validate only safe static references/expressions.

Acceptance:

- no authorization/security guarantees are implied;
- permissions and access logic are documented as runtime-sensitive.

### 8.4 REMOVE / ALTER / REBUILD / INFO / SHOW

Complete static target validation for all grammar variants.

Acceptance:

- unknown static target object/table/field diagnostics are emitted;
- implementation-specific result shapes remain partial.

---

## Phase 9: machine-readable output contract

Objective: adapters can consume the CLI/core output without reinterpreting text.

### 9.1 Structured statement and expression facts in JSON

Expose enough JSON for host adapters:

- source id/path;
- statement kind/span;
- response shape;
- params with inferred kinds/spans;
- diagnostics with stable code/severity/span/message/notes;
- partial-analysis reasons;
- optional expression facts where useful for editor tooling.

Acceptance:

- `surrealguard check --json` output is stable and fixture-tested.

### 9.2 Static-analysis bypass and limit docs

Document explicit limits:

- host string concatenation;
- dynamic table/field/function names;
- runtime permissions/auth;
- data-dependent cardinality;
- SurrealDB version drift;
- partial analysis semantics.

Acceptance:

- docs are clear enough that adapters do not claim security guarantees the core does not provide.

---

## Phase 10: readiness fixtures and final audit

Objective: prove core readiness before starting host adapter spike.

Required fixtures:

1. `all_statements.surql`: every statement kind with stable statement facts.
2. `expressions.surql`: literals, objects, arrays, records, paths, params, LETs, operators, casts, functions, subqueries, blocks.
3. `selects.surql`: projections, WHERE, modifiers, grouping, aggregates, graph, fetch, dynamic partials.
4. `mutations.surql`: all mutation forms, payloads, WHERE, RETURN forms, static misuses.
5. `graphs.surql`: relation metadata, graph directions, edge filters/selections, endpoint errors, dynamic partials.
6. `blocks.surql`: LET, RETURN, IF/ELSE, FOR, THROW/BREAK/CONTINUE, nested scopes.
7. `schema_objects.surql`: DEFINE/REMOVE/ALTER/INFO/SHOW coverage.
8. `dynamic_partials.surql`: constructs that must remain unknown/partial.

Final acceptance:

```bash
cargo fmt
cargo fmt --check
git diff --check
cargo test --workspace -- --nocapture
cargo check --workspace
surrealguard check --json fixtures/adapter-readiness/*.surql
```

Only after this passes should we start the host adapter spike.

---

## Remaining execution backlog

Keep working through these in strict TDD slices. Do not start Rust, TypeScript, or other host adapters until Phase 10 passes.

### Immediate spine migration

1. `feat: route select projection expressions through statement env`
   - SELECT aliases, VALUE projections, nested expression projections, and response shapes consume the env snapshot at the SELECT source position.
2. `feat: route where and graph predicates through statement env`
   - WHERE param-kind inference, predicate field refs, graph-local edge predicates, functions, LET vars, and operators share one env-aware expression path.
3. `feat: route mutation return expressions through statement env`
   - mutation RETURN fields/expressions/functions use schema row context plus LET/env facts.
4. `feat: generalize expression diagnostics entrypoint`
   - replace function-call-named walkers with a single expression diagnostics dispatcher for functions, binary/unary operators, casts, subqueries, blocks, IF/FOR expressions, and partial reasons.

### Expression fact completion

5. Scalar literals: none/null, decimal, datetime, duration, uuid, bytes, record IDs/things.
6. Operators: boolean, comparison, arithmetic unary, coalesce/default, regex/contains where grammar exposes them.
7. Casts and `type::*` functions, after SurrealDB smoke tests.
8. Subqueries and block expressions with child statement sequences and explicit partials for multi-result/data-dependent cases.

### Function completion

9. Expand verified built-in signature groups: math, string, array, object, time, record/type.
10. Apply function diagnostics in every expression position: LET, RETURN, mutation values, IF/FOR conditions, WHERE, DEFINE defaults/bodies.
11. Index `DEFINE FUNCTION` signatures and statically check known user-function calls.

### SELECT completion

12. Analyze ORDER BY, LIMIT, START, TIMEOUT, PARALLEL, WITH/EXPLAIN operands where expression-like.
13. Verify/model GROUP BY, GROUP ALL, aggregates, and non-grouped field behavior.
14. Smoke-test and model or explicitly partialize SPLIT, EXPLAIN, and every accepted shape-changing modifier.

### Mutation completion

15. Complete mutation WHERE and assignment expression analysis for SET, CONTENT/MERGE/REPLACE, PATCH, WHERE, and tuple INSERT.
16. Complete mutation RETURN expression/function aliases, unknown-field validation, DIFF conservatism, and schema row-context shape inference.

### Graph completion

17. Complete outbound, inbound, bidirectional, multi-hop, and parenthesized traversal direction/endpoint resolution.
18. Complete edge-local WHERE/SELECT/projection/params/functions/operators/response shapes with distinct edge vs target row contexts.
19. Strengthen FETCH and nested record materialization, with dynamic record tables as explicit partials.

### Control-flow completion

20. Verify/model FOR loop variable kinds, body envs, RETURN/THROW/BREAK/CONTINUE effects, result shapes, and loop-local scoping.
21. Analyze THROW, BREAK, CONTINUE, SLEEP, KILL, USE, OPTION, and transaction statements beyond name-only facts where statically meaningful.
22. Generalize nested IF/ELSE, ELSE IF, missing-ELSE fallthrough partials, and return-shape unioning.

### Schema object completion

23. Index DEFINE PARAM kinds/defaults and use them for later `$param` references.
24. Validate DEFINE INDEX/EVENT/ANALYZER static references and expressions.
25. Record DEFINE ACCESS/USER/SCOPE/TOKEN/permission facts without implying runtime auth guarantees.
26. Complete REMOVE/ALTER/REBUILD/INFO/SHOW static target validation and partial result shapes.

### Output/readiness completion

27. Stabilize JSON for statement facts, expression facts where useful, response shapes, params, diagnostics, and partial reasons.
28. Document bypasses and limits: host concatenation, dynamic names, runtime auth, data-dependent cardinality, SurrealDB version drift, partial semantics.
29. Add adapter-readiness fixtures: all_statements, expressions, selects, mutations, graphs, blocks, schema_objects, dynamic_partials.
30. Final gate: `cargo fmt`, `cargo fmt --check`, `git diff --check`, `cargo test --workspace -- --nocapture`, `cargo check --workspace`, and `surrealguard check --json fixtures/adapter-readiness/*.surql`.

Only after this passes should we start the host adapter spike.
