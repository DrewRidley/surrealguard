# Diagnostics architecture: analyzers self-emit at the point of detection

Status: agreed in principle, amended per the inference-never-checks ruling
(2026-07-05). Implementation gated on the open questions at the end.

## Problem

Type inference currently *detects* problems with complete information and
then discards everything except a poison type. `evaluate()` knows exactly
which argument of which call mismatched which expected kind — and returns
`Kind::Any`. The SELECT analyzer knows the `FROM` table isn't in the schema —
and returns `Kind::Any`. If diagnostics were built as a separate pass, every
one of these detections would have to be *re-implemented* somewhere else,
against the same AST, producing the same conclusion. That is the current
shape of the frozen validators in `semantic.rs`: a parallel detection engine
that re-derives what inference already knew.

## Architecture

Five rules:

1. **Detection site = emission site.** The analyzer that discovers a problem
   emits the finding, through the `AnalysisContext` it already holds. No
   separate validation pass, no re-detection. Inference and diagnosis are
   one walk.

2. **One entry point.** `analyze any snippet → (types, findings)`. Callers
   (CLI, LSP, tests) invoke one function per source; everything else —
   lowering, per-statement dispatch, emission — happens inside. The
   accumulated findings come back alongside the inferred kinds.

3. **Hierarchical awareness by span overlap.** A composite analyzer (block,
   IF, transaction) that needs to know whether an inner statement produced
   findings does not need a result channel: findings carry spans, and the
   parent's span contains the child's. A `findings_in(range)` query over the
   accumulated collection answers "did anything inside me fail?". (Built
   when the first consumer exists — nothing speculative.)

4. **Findings and types are independent.** Inference never checks: a
   mistaken call still has its return type (`string::len(42)` infers `int`
   *and* gets an argument-kind finding). `Kind::Any` appears only where the
   type is genuinely unknowable — and most of those cases (unknown table,
   unknown field) coincide with a finding, while most findings (argument
   mismatches, operand mismatches, non-bool conditions) coincide with a
   perfectly known type. Emission is purely additive to inference.

5. **No double reporting.** A detection site never emits while its old
   `semantic.rs` counterpart still runs. Enabling emission at a new site and
   deleting the old validator happen in the same change, with the existing
   `analysis.rs` finding-assertions as the equivalence pin.

Consequences for existing code:

- The pure `*_response_kind(...)` cores merge back into their `analyze_*`
  entry points: emission needs the context, and the context is now genuinely
  load-bearing (schema, env, row table, diagnostics — no speculative
  fields). Tests construct a context; asserting on `(kind, findings)` pairs
  is the natural test shape.
- The invariant data already exists declaratively and unchanged by this
  phase: `Signature`'s arity/argument expectations (all 373 files),
  `binary_operands_compatible`, `kind_is_assignable_to`. Emission *reads*
  them; inference continues to ignore them.
- A checking pass over the signature table needs `(ctx, call)` — spans per
  argument via `call.args[i].span` (the reason `ast::Call` carries spanned
  argument expressions). `evaluate()` itself stays inference-only.
- Parse-level breakage (`Statement::Partial` from `ERROR`/`MISSING`) emits
  **nothing** in analyzers: the parser already reported it. Analyzer
  emission is exclusively for well-formed syntax with semantic problems.

## Detection-site inventory

Every place inference currently swallows a detection, mapped to the old
validator it retires (codes are the existing families; final numbers get
assigned during implementation).

| # | Site (new analyzer tree) | Detects | Retires (semantic.rs / schema.rs) | Code family |
|---|---|---|---|---|
| 1 | `function::signature::evaluate` | wrong argument count | `validate_function_calls` (arity arm) | function |
| 2 | `function::signature::evaluate` | wrong argument kind (per-arg span) | `validate_function_calls` (kind arm) | function |
| 3 | `function::mod` dispatch | unknown builtin path | `validate_function_call` (unknown name) | function |
| 4 | `select` FROM / mutation targets | table not in schema | `collect_table_reference_diagnostics` | schema |
| 5 | `select` projections (`kind_for_path` miss) | unknown field on table | `validate_select_projection_fields` | schema |
| 6 | `select`/`relate` graph chains | edge not a relation / endpoint mismatch / odd hop count | `validate_select_graph_references`, `validate_graph_references_for_relate_statement` | graph |
| 7 | `mutation` RETURN fields | unknown field in RETURN projection | `validate_mutation_return_*` | schema |
| 8 | `infer` binary expressions | operand kind mismatch (`string > int`; the comparison still infers `bool`) | `validate_binary_expression` | type_error |
| 9 | `flow::if_else` conditions | non-bool condition | `validate_if_conditions` | type_error |
| 10 | `mutation` SET assignments | value not assignable to field kind | `validate_mutation_value_assignability`, `validate_assignment_fields_on_table` | type_error/schema |
| 11 | thin statements (LIVE/ALTER/REBUILD/SHOW/INFO) | table not in schema | `collect_table_reference_diagnostics` (thin arms) | schema |

New detections inference already makes that have **no** old counterpart
(new findings, net coverage gain):

| # | Site | Detects | Proposed severity |
|---|---|---|---|
| 12 | graph steps | multi-target step (`->(a, b)`) unresolvable to one table | warning |
| 13 | `infer` casts | cast to unknown type name | error |
| 14 | `select` VALUE | `SELECT VALUE` with multiple projections | error |
| 15 | `infer` arrays | mixed-kind array literal (currently a partial fact) | lint/info |
| 16 | mutations | `ONLY` with multi-row target (e.g. whole-table CREATE ONLY) | warning |

Detections that are **deliberately not findings** (poison without emission
is correct):

- Schemaless tables / fields with no declared type — genuine unknowns, not
  user errors at the query site.
- Unbound `$params` — an input to param inference (a feature output), not a
  defect. Host integrations supply them.
- Unmodeled constructs (closures, Tier-2 clauses) — analyzer limitations,
  not user errors. Silent `Any`, listed in docs.
- `Statement::Partial` from broken syntax — already reported by the parser.

Old validators with **no** new-tree home yet (they stay until their
statement's invariants are formalized): param-kind inference from
predicates, `validate_object_fields_on_table`, insert column checks,
`validate_events`/index/alter target checks in `schema.rs` (these belong to
the DEFINE-family analyzers).

## Suggested order of implementation

1. Entry point + context plumbing (merge pure cores into `analyze_*`,
   `evaluate(ctx, call, ...)`) — no emission yet, suite stays green.
2. Sites 1–3 (function family): first emissions, retire
   `validate_function_calls`.
3. Sites 4–7 (schema/graph family), statement by statement.
4. Sites 8–10 (type errors).
5. Site 11 + the leftover schema-statement validators.
6. New findings 12–16, each with its own code and test.

Each step deletes its `semantic.rs` counterpart in the same change; the old
engine erodes to zero by the end of step 5, at which point `select_ir.rs`,
the node-based `expression.rs`, and the keyword scanners have no callers and
`tree-sitter` leaves the workspace crate.

## Open questions

1. **Severity model** — *mechanism ruled (2026-07-07), defaults still open.*
   Findings carry only their intrinsic class (error/warning/info) from the
   catalog — the rustc model. Presentation policy (`warnings_as_errors`,
   per-code lint levels, suppression) applies at the consumption edges:
   CLI (exit codes + rendering), LSP (severity mapping), host adapters.
   `Finding::effective_severity` was deleted — it was a policy output baked
   into data, and nothing ever set it. Non-lint classes can be promoted by
   policy but never demoted; lints take allow/warn/deny. What remains open
   is only the per-code *default* stamps in the catalog.
2. ~~Finding code allocation~~ **Ruled (2026-07-07): retired validators are
   removed fully.** New emission sites get their own codes and messages;
   nothing is preserved for compatibility. The old finding-tests are
   rewritten against the new findings when their site flips.
3. ~~Message style~~ **Ruled with 2: fresh messages, standardized once.**
4. **`findings_in(range)`** — agreed as the hierarchy mechanism, built with
   its first consumer. The likely first consumer is transaction-awareness
   (`BEGIN`/`COMMIT` blocks reporting aggregate validity).

## Note (2026-07-06): value-dependent builtins as parameter constraints

Const-value tracking (`ExpressionFact.value: Option<surrealdb_types::Value>`)
resolves value-dependent builtins statically when their arguments trace to
literals (`type::field('name.first')`, including through `LET`). Two
follow-ons belong to later phases:

- **Diagnostics**: when the const value is known and *invalid* — a
  `type::field` path naming no schema field — that is a statically-provable
  invariant violation and should be a finding.
- **Host adapters**: when the argument is a host-supplied parameter
  (`type::field($param)` with no binding), the analysis knows a constraint
  pair: the parameter's *type* (string) and its *value domain* (the row
  table's field paths). Exporting that lets host bindings (Rust/TypeScript)
  check call sites at their compile time. Parameter constraint export is a
  host-adapter design item.

## Readiness checklist (2026-07-07)

Prerequisites satisfied:
- Type inference covers every statement kind, all builtins, closures,
  subqueries, const values; 373 tests pin it.
- Inference and checking are structurally separated; invariant data is
  declarative and waiting.
- Spans exist at every emission site (per argument, per arrow, per field).
- The retirement map (old validator ↔ new emission site) is the table above.

Blocking on decisions, not work:
- Open questions 1–4 below (severity defaults, code allocation, message
  style, `findings_in`'s first consumer).
- Both repositories carry the entire migration uncommitted; a baseline
  commit precedes phase work.
