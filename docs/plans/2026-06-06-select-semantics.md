# SELECT Semantics Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Build comprehensive SELECT semantic analysis for SurrealQL using tree-sitter CST nodes as the syntax authority and SurrealDB's own Rust types for value/kind facts. Do not invent a parallel `surrealguard-types` type system.

**Architecture:** The analyzer lowers each tree-sitter `SelectStatement` into an internal semantic IR that keeps node kind, byte span, and source text for every SELECT component. The IR is not a replacement AST and not a custom type system; it is a small analysis index over the tree-sitter CST. Result-shape inference produces `ResponseShape` facts for diagnostics/adapters, with leaf scalar kinds represented by `surrealdb::types::Kind` and runtime-value interoperability represented by `surrealdb::types::Value`, `RecordId`, `Object`, `Array`, `Table`, etc. Host adapters then map this response shape into Rust macro output, TypeScript transformer output, LSP hovers, CLI JSON, or MCP facts.

**Tech Stack:** Rust, `tree-sitter`, local `tree-sitter-surrealql`, `surrealguard-syntax`, `surrealdb = { version = "3.1.3", default-features = false }` via the public `surrealdb::types` re-export, `surrealguard-diagnostics`, and `surrealguard-workspace`.

---

## Hard rules from design review

1. No `surrealguard-types` crate in the maintained design.
2. No custom SurrealQL parser or custom AST for syntax; tree-sitter nodes are the syntax source of truth.
3. No parallel enum for SurrealDB scalar/data kinds; use `surrealdb::types::Kind` for schema field kinds and expression kind facts.
4. No fake precision. Dynamic or unsupported SELECT forms must emit explicit partial-analysis facts and diagnostics instead of claiming a precise response shape.
5. Every SELECT feature lands test-first, with grammar/CST tests and semantic/result-shape tests.
6. Diagnostics and result-shape inference consume the same SELECT IR so CLI, LSP, Rust proc macro, TypeScript transformer, and MCP behavior cannot drift.

## Verified parser facts

From `../tree-sitter-surrealql/grammar.js`:

- `SelectStatement` is:
  - `SELECT`
  - `Fields`
  - optional `OmitClause`
  - `FROM`
  - optional `ONLY`
  - either nested statement or `csep($._value), repeat($._modifierClause)`
- `Fields` is either:
  - `VALUE Predicate`
  - comma-separated `_inclusivePredicate`
- `_inclusivePredicate` is either `Any` (`*`) or `Predicate`.
- `Predicate` is either `_value` or `_value AS Ident`.
- `OmitClause` is `OMIT csep(_inclusivePredicate)`.
- SELECT modifiers include `WithClause`, `WhereClause`, `SplitClause`, `GroupClause`, `OrderClause`, `LimitStartComboClause`, `FetchClause`, `TimeoutClause`, `ParallelClause`, `TempfilesClause`, `ExplainClause`, `VersionClause`, and `ReturnClause`.
- `FetchClause` is `FETCH csep(Idiom)`.
- `ReturnClause` is `RETURN BEFORE | AFTER | DIFF | Fields`.
- Graph traversal is represented through path/lookup nodes including `Lookup`, `LookupRight`, `LookupLeft`, `LookupBoth`, `LookupSelection`, `GraphFieldSelection`, `GraphPredicate`, and graph-local `WhereClause` / split / group / order / limit aliases.

Implementation must query these CST node kinds directly. Do not string-split `->`, `<-`, `<->`, `OMIT`, `AS`, or `RETURN`.

## Core data model, without custom DB types

### `SelectIr` private analysis structs

Private workspace-only structs are allowed when they describe analysis relationships, not database types:

- `SelectIr<'tree>`
  - `statement: Node<'tree>`
  - `fields: SelectFields<'tree>`
  - `omit: Vec<SelectProjection<'tree>>`
  - `sources: Vec<SelectSource<'tree>>`
  - `only: Option<SourceSpan>`
  - `modifiers: Vec<SelectModifier<'tree>>`
  - `limitations: Vec<PartialReason>`
- `SelectProjection<'tree>`
  - original `Predicate` or `Any` node
  - expression node
  - optional alias `Ident` node
  - output key policy (`Wildcard`, `FieldPath`, `Alias`, `ExpressionText`, `Dynamic`)
- `SelectSource<'tree>`
  - table ident, record id, variable, array, subquery, graph lookup, expression, or unknown
  - source span and owning CST node
- `GraphTraversal<'tree>`
  - direction per hop from `LookupRight`, `LookupLeft`, `LookupBoth`
  - relation predicate node (`Ident`, `Any`, or `LookupSelection`)
  - optional graph-local field selection, filters, ordering, limits, alias
- `SelectModifier<'tree>`
  - exact modifier CST node plus enum tag for WHERE/FETCH/RETURN/etc.

These structs can expose helper methods, but they must preserve the original tree-sitter node and source span for diagnostics.

### `ResponseShape` analysis facts

`ResponseShape` is an analyzer output contract, not a SurrealDB type replacement:

- `Unknown { reason }`
- `Value { kind: surrealdb::types::Kind }`
- `Record { table: surrealdb::types::Table }`
- `Object { fields: BTreeMap<String, FieldShape>, open: bool }`
- `Array { element: Box<ResponseShape>, max_len: Option<u64> }`
- `Set { element: Box<ResponseShape>, max_len: Option<u64> }`
- `Union { variants: Vec<ResponseShape> }`
- `StatementSet { statements: Vec<ResponseShape> }` if needed later for multi-statement outputs

`FieldShape` contains:

- `shape: ResponseShape`
- `kind: Option<surrealdb::types::Kind>` when the schema kind is known
- `span: SourceSpan` of the schema field declaration
- `materialized_by_fetch: bool`
- `partial: Vec<PartialReason>`

This is intentionally a response schema, not a duplicate of `surrealdb::types::Kind`. Leaves use `Kind`; object/array layout is query response structure needed by tools.

### Schema facts

Schema index facts should store:

- table name as `surrealdb::types::Table` where possible
- record/table kinds as `surrealdb::types::Kind::Record` / `Kind::Table`
- scalar field kinds as `surrealdb::types::Kind`
- original CST node and source span for all definitions
- relation metadata: relation table, `IN` table set, `OUT` table set, edge fields, spans

If SurrealDB's public types cannot represent some analysis-only metadata, store metadata as source spans, strings, and tree-sitter node references. Do not create a second scalar/type hierarchy.

## SELECT variant matrix and expected behavior

### A. Source and cardinality

| Query | Expected result shape | Diagnostics / partials |
| --- | --- | --- |
| `SELECT * FROM person` | `Array<Object<person fields>>` | unknown table `E1003`; schemaless table = open object partial |
| `SELECT * FROM ONLY person:one` | `Object<person fields>` | if record table known |
| `SELECT * FROM person:one` | `Array<Object<person fields>>` unless SurrealDB docs/tests prove scalar behavior without `ONLY` | document verified behavior |
| `SELECT * FROM $records` | `Array<Unknown(dynamic-source)>` | collect `$records`; no precision until host param type maps it |
| `SELECT * FROM [person:one, person:two]` | `Array<Object<person fields>>` | partial if mixed tables |
| `SELECT * FROM [person:one, product:two]` | `Array<Union<Object<person>, Object<product>>>` or partial if union support deferred | no bogus single table |
| `SELECT * FROM (SELECT * FROM person)` | nested SELECT shape feeds source if static | partial until subquery shape plumbing lands |
| `SELECT * FROM type::table($name)` | `Array<Unknown(dynamic-source)>` | dynamic source partial |

Tests:

- `select_star_from_table_expands_known_fields`
- `select_only_record_returns_single_row_shape`
- `select_record_without_only_documents_cardinality`
- `select_variable_source_is_dynamic_unknown_and_collects_param`
- `select_array_of_same_table_records_infers_table_shape`
- `select_array_of_mixed_record_tables_is_union_or_partial`
- `select_subquery_source_is_partial_until_nested_shape_supported`

### B. Projection forms

| Query | Expected result shape | Notes |
| --- | --- | --- |
| `SELECT * FROM person` | all known fields, `open=false` if schema is strict/known, otherwise `open=true` | include table id policy as separate decision |
| `SELECT name, age FROM person` | object with `name`, `age` | field order not semantically important, but spans are |
| `SELECT profile.email FROM person` | initially key `profile.email`; later nested object once nested shape is implemented | validate full field path if schema has it |
| `SELECT name AS display_name FROM person` | object with `display_name` | kind comes from `name` |
| `SELECT 1 AS one FROM person` | object with `one: Kind::Int/Number` if literal inference exists, else unknown expression | do not require field lookup |
| `SELECT count() AS total FROM person GROUP ALL` | `total` unknown until function signatures are wired | group all is shape-changing partial if aggregate semantics are unknown |
| `SELECT VALUE name FROM person` | `Array<Value{Kind::String}>` | VALUE unwraps object row projection |
| `SELECT VALUE count() FROM person` | `Array<Unknown(expression)>` initially | later function signatures can refine |
| `SELECT DIFF FROM person` | partial/unsupported unless grammar exposes it as SELECT fields in current version | do not silently ignore |

Tests:

- `select_named_fields_infers_projected_object_shape`
- `select_nested_field_preserves_field_path_key_initially`
- `select_alias_renames_projected_field`
- `select_alias_of_literal_uses_expression_shape_or_partial`
- `select_value_field_returns_array_of_field_kind`
- `select_value_expression_returns_unknown_until_expression_inference`
- `select_expression_without_alias_uses_stable_expression_key_or_partial`

### C. OMIT

| Query | Expected result shape | Diagnostics / partials |
| --- | --- | --- |
| `SELECT * OMIT password FROM user` | wildcard object minus `password` | unknown omit field = `E1005` or lint depending policy |
| `SELECT * OMIT profile.secret FROM user` | remove exact dotted key initially | nested removal later |
| `SELECT name OMIT password FROM user` | no result change | lint later: omitted field not in projection |
| `SELECT * OMIT $dynamic FROM user` | object is partial/open | dynamic omit key partial |
| `SELECT * OMIT password, token FROM user` | remove both | spans per omitted field |

Tests:

- `select_omit_removes_wildcard_field`
- `select_omit_multiple_fields_remove_each_field`
- `select_omit_unknown_field_reports_contextual_diagnostic`
- `select_omit_on_non_wildcard_projection_does_not_drop_known_field`
- `select_dynamic_omit_marks_shape_partial`

### D. WHERE and row-context expression validation

| Query | Expected behavior |
| --- | --- |
| `SELECT * FROM person WHERE age > 18` | validate `age` exists in row context and comparison is plausible |
| `SELECT * FROM person WHERE missing = true` | diagnostic on `missing` with WHERE context |
| `SELECT * FROM person WHERE friend = $friend` | collect/refine `$friend` from field kind if possible |
| `SELECT * FROM person WHERE profile.email CONTAINS '@'` | validate dotted field path |
| `SELECT * FROM person WHERE $dynamic` | collect param; no field diagnostic |
| `SELECT * FROM person WHERE tags CONTAINS 'rust'` | validate `tags`; operator compatibility later |
| `SELECT * FROM person WHERE count(posts) > 0` | partial expression inference until builtins are modeled |

Tests:

- `select_where_validates_known_field_references`
- `select_where_reports_unknown_field_references`
- `select_where_dotted_field_uses_schema_path`
- `select_where_collects_param_and_refines_from_field_comparison`
- `select_where_dynamic_param_does_not_emit_field_diagnostic`
- `select_where_function_expression_is_partial_not_error`

### E. FETCH

FETCH does not change top-level cardinality. It changes materialization of projected or wildcard fields.

| Query | Expected result shape | Diagnostics / partials |
| --- | --- | --- |
| `SELECT * FROM person FETCH friend` | `friend` record field materializes as object shape for target table | if field is `Kind::Record([person])` |
| `SELECT friend FROM person FETCH friend` | projected `friend` materialized | keep `materialized_by_fetch=true` |
| `SELECT friends FROM person FETCH friends` | array/set of records materializes element shape | if schema says array/set record |
| `SELECT * FROM person FETCH missing` | unknown fetch field diagnostic | span on `missing` |
| `SELECT * FROM person FETCH profile.best_friend` | validate dotted path and materialize leaf | partial if intermediate object model absent |
| `SELECT * FROM person FETCH $dynamic` | grammar likely prevents this; if parsed, dynamic partial | no panic |

Tests:

- `select_fetch_materializes_record_field_shape`
- `select_fetch_materializes_projected_record_field`
- `select_fetch_materializes_array_of_records`
- `select_fetch_unknown_field_reports_diagnostic`
- `select_fetch_dotted_record_path_validates_each_segment`
- `select_fetch_non_record_field_reports_non_materializable_or_partial`

### F. Graph traversal and `->` / `<-` / `<->`

Graph traversal must be parsed from `Lookup*` and `LookupSelection` CST nodes.

| Query | Expected behavior |
| --- | --- |
| `SELECT ->likes->post FROM person` | validate relation `likes`, source endpoint `person`, target `post`; row shape is target table or traversal array per verified semantics |
| `SELECT <-likes<-person FROM post` | reverse traversal validates `OUT post` / `IN person` compatibility |
| `SELECT <->likes<->person FROM person` | bidirectional traversal validates either endpoint compatibility |
| `SELECT ->likes->post.title AS liked_title FROM person` | validate relation, target table, target field `title`; output alias `liked_title` |
| `SELECT ->missing->post FROM person` | `E3001` unknown edge table |
| `SELECT ->likes->missing FROM person` | unknown target table diagnostic |
| `SELECT ->likes[WHERE weight > 0.5]->post FROM person` | graph WHERE field context is edge table `likes`, not source/target row |
| `SELECT ->likes[WHERE missing > 0.5]->post FROM person` | unknown edge field diagnostic |
| `SELECT ->(SELECT weight FROM likes WHERE weight > 0.5)->post FROM person` | graph field selection / lookup selection is partial until modeled |
| `SELECT ->likes.* FROM person` | wildcard target/edge projection must be explicit about context |

Tests:

- `select_graph_traversal_validates_relation_and_target`
- `select_reverse_graph_traversal_uses_relation_in_endpoint`
- `select_bidirectional_graph_traversal_accepts_either_endpoint`
- `select_graph_projection_validates_target_field`
- `select_graph_projection_alias_sets_output_key`
- `select_unknown_graph_edge_reports_diagnostic`
- `select_unknown_graph_target_reports_diagnostic`
- `select_graph_where_validates_edge_fields`
- `select_graph_where_does_not_validate_against_source_fields`
- `select_graph_lookup_selection_is_partial_not_panic`

### G. RETURN clause on SELECT

The grammar currently accepts `ReturnClause` as a SELECT modifier: `RETURN BEFORE | AFTER | DIFF | Fields`. Before implementing strong semantics, verify actual SurrealDB behavior with docs or a local/remote SurrealDB smoke test.

Initial analyzer policy:

- Parse and preserve `ReturnClause` in `SelectIr`.
- `RETURN Fields` should be treated like a shape-changing projection only after verified.
- `RETURN BEFORE`, `RETURN AFTER`, and `RETURN DIFF` on SELECT should either map to verified SurrealDB behavior or emit an explicit unsupported/invalid diagnostic.
- Never ignore `RETURN` on SELECT.

Tests:

- `select_return_clause_is_preserved_in_ir`
- `select_return_fields_is_partial_until_verified`
- `select_return_before_after_diff_emit_verified_behavior_or_diagnostic`
- `select_return_clause_does_not_get_confused_with_return_statement`

### H. AS keyword

`AS` appears in normal projections and graph lookup selection aliases.

Tests:

- `select_projection_as_alias_uses_alias_span_for_output_key`
- `select_expression_as_alias_does_not_require_field_name`
- `select_graph_lookup_as_alias_is_separate_from_projection_alias`
- `select_alias_collision_reports_or_deterministically_overwrites_per_policy`

Policy decision needed before implementation: if two projections produce the same output key, either last-write-wins matching SurrealDB or diagnostic. Verify with SurrealDB before finalizing.

### I. Modifiers that mostly preserve row shape

These modifiers should be represented in IR and tested to preserve shape unless verified otherwise:

- `WHERE`
- `ORDER BY`
- `LIMIT` / `START` (cardinality may be max-bounded later but row shape unchanged)
- `TIMEOUT`
- `PARALLEL`
- `TEMPFILES`
- `VERSION`
- `WITH NOINDEX` / `WITH INDEX ...`

Tests:

- `select_order_by_preserves_projected_shape_and_validates_order_field`
- `select_limit_start_preserve_row_shape`
- `select_timeout_parallel_tempfiles_preserve_row_shape`
- `select_version_preserves_row_shape_or_marks_temporal_partial`
- `select_with_index_preserves_row_shape`

### J. Shape-changing / advanced modifiers

These must start as explicit partials unless implemented with verified semantics:

- `SPLIT`
- `GROUP BY`
- `GROUP ALL`
- aggregate projections
- `EXPLAIN` / `EXPLAIN FULL`
- nested subqueries
- dynamic functions and dynamic table names

Tests:

- `select_group_all_marks_aggregate_shape_partial_without_wrong_row_shape`
- `select_group_by_marks_shape_partial_until_group_schema_supported`
- `select_split_marks_shape_partial_until_split_schema_supported`
- `select_explain_returns_explain_shape_or_partial_after_verification`
- `select_unsupported_modifier_does_not_panic`

## Diagnostic codes

Keep existing codes stable where possible, but add only when tests require:

- `E1003`: unknown table/source reference
- `E1004`: unknown projected field on table
- `E1005`: unknown field in contextual clause (`WHERE`, `OMIT`, `FETCH`, order)
- `E1006`: non-record field used in `FETCH` materialization context, if treated as error
- `E2001`: expression kind mismatch, later
- `E3001`: unknown graph relation table
- `E3002`: graph endpoint incompatible with source/target table
- `E3003`: unknown graph target table
- `E3004`: unknown edge field in graph-local filter/projection
- `W6001`: partial result-shape inference
- `W6002`: unsupported SELECT construct preserved but not semantically modeled

Every diagnostic must include the smallest useful tree-sitter node span.

## Implementation phases

### Phase 0: remove the custom type-system direction

Objective: stop extending `surrealguard-types` and pivot the maintained design to tree-sitter + SurrealDB public types.

Tasks:

1. Add `surrealdb = { version = "3.1.3", default-features = false }` to workspace dependencies.
2. Introduce `ResponseShape` in `crates/workspace/src/response_shape.rs` or `crates/semantics` if a semantics crate is created.
3. Use `surrealdb::types::Kind`, `Table`, `RecordId`, `Object`, `Array`, `Value` where applicable.
4. Replace `StatementAnalysis.result_type` with `result_shape: Option<ResponseShape>`.
5. Replace field schema kind storage with `surrealdb::types::Kind`.
6. Keep any removed `surrealguard-types` tests only as historical reference; do not port its type hierarchy.

Tests:

- `schema_field_type_string_maps_to_surrealdb_kind_string`
- `schema_field_type_record_person_maps_to_surrealdb_kind_record`
- `statement_analysis_uses_response_shape_not_surrealguard_type`

Verification:

```bash
cargo test -p surrealguard-workspace -- --nocapture
cargo check --workspace
git grep -n "surrealguard-types\|surrealguard_types" -- . ':!target' ':!docs/archive/**'
```

Expected: no maintained-code references after the migration phase is complete.

### Phase 1: SELECT IR skeleton

Objective: one tree-sitter-backed IR path for all SELECT diagnostics and shape facts.

Tasks:

1. Add private `select_ir` module under workspace/semantics.
2. Add CST extraction tests for `Fields`, `OmitClause`, `FROM`, `ONLY`, modifiers, and graph `Lookup` nodes.
3. Route current table-reference and projected-field validation through `SelectIr`.
4. Preserve existing behavior before adding new semantics.

Tests:

- `select_ir_extracts_fields_omit_source_only_and_modifiers`
- `select_ir_preserves_projection_alias_nodes`
- `select_ir_extracts_fetch_where_return_modifiers`
- `select_ir_extracts_graph_lookup_nodes_without_string_splitting`

### Phase 2: simple table sources and projections

Objective: infer response shapes for `SELECT *`, named fields, aliases, and `VALUE` from simple table/record sources.

Tests to implement first:

- `select_star_from_table_expands_known_fields`
- `select_named_fields_infers_projected_object_shape`
- `select_nested_field_preserves_field_path_key_initially`
- `select_alias_renames_projected_field`
- `select_value_field_returns_array_of_field_kind`
- `select_only_record_returns_single_row_shape`
- `select_unknown_table_keeps_shape_unknown`
- `select_schemaless_table_returns_open_unknown_object_shape`

### Phase 3: OMIT and row-preserving modifiers

Objective: model OMIT and validate row-context field references in modifiers that preserve shape.

Tests:

- OMIT tests from section C
- WHERE tests from section D
- ORDER/LIMIT/TIMEOUT/PARALLEL tests from section I

### Phase 4: FETCH materialization

Objective: materialize record fields using schema relation between field kind and target table.

Tests:

- FETCH tests from section E

### Phase 5: relation schema and graph traversal

Objective: index relation tables and validate graph traversals, including graph-local WHERE contexts.

Tasks:

1. Extract `DEFINE TABLE edge TYPE RELATION IN a OUT b` metadata.
2. Preserve relation direction spans.
3. Build graph traversal IR from `Lookup*` nodes.
4. Validate source/edge/target compatibility.
5. Infer target response shape for simple graph projections.

Tests:

- Graph tests from section F

### Phase 6: RETURN and advanced modifiers

Objective: document/verify SurrealDB behavior for RETURN, GROUP, SPLIT, EXPLAIN, nested subqueries, and aggregate projections; implement only verified semantics.

Tests:

- RETURN tests from section G
- advanced modifier tests from section J

### Phase 7: host adapter contract tests

Objective: prove one semantic engine feeds host-language integrations.

Tests:

- Rust proc macro fixture maps `ResponseShape` to Rust compile-time output expectations.
- TypeScript transformer fixture maps `ResponseShape` to TS type text expectations.
- LSP hover/diagnostics fixture uses the same `ResponseShape` JSON.
- CLI `--json` includes stable response-shape output for SELECT.

## Test organization

Create dedicated tests rather than stuffing everything into `analysis.rs`:

- `crates/workspace/src/tests/select_ir.rs`
- `crates/workspace/src/tests/select_sources.rs`
- `crates/workspace/src/tests/select_projection.rs`
- `crates/workspace/src/tests/select_omit_where.rs`
- `crates/workspace/src/tests/select_fetch.rs`
- `crates/workspace/src/tests/select_graph.rs`
- `crates/workspace/src/tests/select_modifiers.rs`

Shared helpers:

- `workspace_with(query: &str) -> (WorkspaceAnalysis, SourceId)`
- `select_statement(output, source) -> &StatementAnalysis`
- `select_shape(output, source) -> &ResponseShape`
- `assert_object_field(shape, field, kind)` where `kind` is `surrealdb::types::Kind`
- `assert_partial(shape, reason)`
- `assert_finding(output, code, message_substring)`

## Verification gates per commit

For touched Rust files:

```bash
rustfmt --edition 2021 <touched-rust-files>
rustfmt --edition 2021 --check <touched-rust-files>
cargo test -p surrealguard-workspace -- --nocapture
cargo test --workspace -- --nocapture
cargo check --workspace
git diff --check
git status --short
```

For doc-only planning commits:

```bash
git diff --check
git grep -n "SurrealGuard-owned type model\|surrealguard-types\|surrealguard_types" docs README.md crates || true
```

Any remaining references must clearly identify legacy/current-code cleanup, not endorse a custom type model.

## Commit boundaries

1. `docs: pivot select plan to tree-sitter and surrealdb types`
2. `refactor: replace custom type output with response shapes`
3. `test: add select ir cst extraction specs`
4. `refactor: route select validation through ir`
5. `feat: infer simple select response shapes`
6. `feat: support select aliases value and omit`
7. `feat: validate select where and row modifiers`
8. `feat: materialize select fetch fields`
9. `feat: index relation tables for graph traversal`
10. `feat: analyze select graph traversals`
11. `feat: handle return and advanced select partials`

Do not start phase 2 until phase 0 and phase 1 prevent a second type system from reappearing.
