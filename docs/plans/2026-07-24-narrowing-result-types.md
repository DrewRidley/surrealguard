# Narrowing-based result types

Status: design / proposal
Author: SurrealQL Analyzer principal engineering
Date: 2026-07-24
Supersedes: nothing (net-new capability); depends on the flow engine in `crates/workspace/src/analyzer/flow/`

---

## 1. Motivation and the core idea

Today SurrealQL Analyzer computes a query's result type as the **schema shape**: for `SELECT name, email FROM user`, `select_response_kind` (`crates/workspace/src/analyzer/data/select.rs:25`) builds `Kind::Array(Kind::Literal(Object({name: string, email: option<string>})))` straight from each field's declared `FieldDef.kind`. The WHERE clause is thrown away for typing purposes — `check_where_clause` (`select.rs:197`) literally comments *"The WHERE kind is irrelevant to the response"* and only emits findings. Permissions never touch the type at all.

That is correct but coarse, and it is wrong in the two directions a real SurrealDB query is *most* precise:

**The `email` example (WHERE tightens).** For
```surql
SELECT name, email FROM user WHERE email != NONE
```
every row SurrealDB returns provably has a non-NONE `email` — the engine keeps a row iff the condition is truthy (`core doc/check.rs::check_where_condition`) and NONE/NULL are never truthy (`core val/mod.rs::is_truthy`). So the honest result type is `Array<{ name: string; email: string }>`, not `email?: string`. Today we emit the loose one.

**The `password` / `$auth.id` example (permission widens).** For
```surql
DEFINE FIELD password ON user TYPE string
  PERMISSIONS FOR select WHERE $auth.id = id;
SELECT email, password FROM user;
```
SurrealDB evaluates the `FOR select` predicate **per surviving row** and *cuts the field* when it is not truthy (`core doc/reduce.rs::reduce_document`, lines 217–267). Across a result set `$auth.id = id` is true for the caller's own row and false for the others, so `password` is present on some rows and absent on others. The honest type is `password?: string`, but the schema says `password: string` — TS callers are told a field is always present that the database routinely omits. That is not imprecision; it is a wrong type.

**Core idea.** A query result is a *relation* — a set of rows. The result type is the **narrowed row shape**: the schema Kind refined by every fact that provably holds of *every row in the result set*. WHERE clauses and control flow **narrow** (remove cases: strip `none`, collapse a record union); per-row permissions **widen** (add `none`). Both are the same operation on one object literal, governed by one soundness rule. When a refinement cannot be *proven*, we emit the schema Kind unchanged — a wrong result type is strictly worse than an imprecise one.

---

## 2. The unified model

### 2.1 One narrowed response kind

There is exactly one output contract: `StatementAnalysis.response_kind` → `surrealql_analyzer_codegen::ts_type` (generation) and `render_kind` (hover). We do **not** add a parallel "narrowed kind" field — MEMORY's `no_speculative_state_channels` rule forbids a second channel with no distinct reader, and there is only one reader. Instead **the narrowed kind *is* `response_kind`**: narrowing is a post-pass applied *inside* `select_response_kind`, over the already-built `row_kind` object literal, before it is wrapped in the outer `Kind::Array`.

The narrowed response kind is computed from four inputs:

```
narrowed_row_kind  =  reduce(  schema_row_kind,
                               where_narrowing,      // §3.1  tighten
                               permission_visibility,// §3.2  widen / omit
                               flow_narrowing )      // §3.3  exit-set union
```

Each source is a function `Kind -> Kind` that either **tightens a leaf that is already present** (WHERE, flow) or **rewrites presence** (permission: optionalize / omit). The schema shape stays structurally recoverable because every source only tightens or annotates existing leaves — so "narrowing off" re-runs the same builder with the post-pass skipped and is provably byte-identical to today's output.

### 2.2 The global SOUNDNESS rule (prove-or-fall-back-to-schema)

> A narrowing may replace a field's schema kind `S` with `N` in the response **only if `N` is a supertype of every runtime value that field can hold in a row the query returns** — i.e. no value the DB can return is excluded from `N`. When this cannot be *proven* from static facts, emit `S` unchanged.

This is enforced *structurally*, not by re-derivation. Every narrowing is expressed as an `Option<Kind>` produced by the existing monotone-tightening transforms in `narrow.rs` (`narrow_kind`, `narrow_out_none`, `narrow_record_to`, `narrow_record_without`). `None` or "unchanged" means "keep `S`". Because P1 reuses transforms already proven sound for the *checking* path, it inherits their soundness rather than re-proving it.

Two corollaries fall out of the widen-vs-narrow asymmetry, and they gate the whole design:

- **Widening is always sound** (`option<T> ⊒ T`): a consumer that handles `T | undefined` is safe even when the runtime always supplies `T`. So per-row permission optionalization can fire freely.
- **Narrowing removes cases** (drop `none`, drop a field, drop a union member): it must be *proven*. So WHERE tightening requires a recognized guard shape, and permission *omission* requires a declared "enforced" perspective (§4, §3.2).

### 2.3 The five soundness spines (one per prior-art body)

| Source | Prior art | Direction | Soundness spine |
|---|---|---|---|
| WHERE | TS occurrence typing; Datalog σ-pushdown | tighten | Every surviving tuple satisfies the predicate, so any fact its TRUE-region proves is a fact about the whole result relation. |
| Literal-eq | Flux/Liquid refinement; Datalog equality prop. | tighten | `f = lit` keeps a row iff `f == lit`; representability gate: apply only if it maps to a tightening `Kind`. |
| Field permission | Postgres RLS + LEFT-JOIN view nullability | widen | A non-total per-row column grant makes the column nullable in the output relation, exactly like an outer-join column. |
| Control flow | TS CFA into RETURN; EdgeDB optional links | union | Result kind = union over reachable exits, each typed in its narrowed env. |
| `$auth` | scoped-session / RLS session context | seed + narrow | `$auth` is engine-supplied `option<record>`; narrow it with the same guards. |

---

## 3. Per-source semantics

### 3.1 WHERE-narrowing (P1)

**Reduction.** A SELECT's WHERE clause is a single **positive** flow-guard over the result set. Every returned row satisfies `WHERE is_truthy`, so the refinements `flow::narrow::positive_effects` proves for an IF's THEN branch hold for the projected row type. **Apply positive polarity only; never negative.**

**Recognized leaf shapes** (everything else is a no-op):

| Predicate | Narrowing | Before → after (field) | Why sound |
|---|---|---|---|
| `f != NONE` / `f IS NOT NONE` | `StripNone` (strip `none` only, **keep `null`**) | `email: string \| undefined` → `email: string` | `f != NONE` = `!f.is_none()`; a NULL has `is_none()==false`, so NULL rows survive — stripping `null` would be unsound. |
| `f != NULL` / `f IS NOT NULL` | strip `null` only, keep `none` | `note: string \| null \| undefined` → `note: string \| undefined` | Symmetric: `NONE != NULL` is TRUE, NONE rows survive. |
| `f = <lit>` (either order) | `Eq(literal_kind)` → `Kind::Literal` | `status: string \| undefined` → `status: "active"` | Surviving rows have `f` exactly `lit`; literal is inherently non-none/non-null. Only if `literal_base` unifies with `f`'s non-none base. |
| `f > lit` / `f >= lit` | `NotNone` (strip none+null) | `age: number \| undefined` → `age: number` | Enum order is `None, Null, Bool, Number, …`; `NONE > 18` is FALSE, so none/null rows are filtered. Value kind itself unchanged (no range types in `Kind`). |
| `f < lit` / `f <= lit` | **NOTHING** (MUST-NOT narrow) | `age: number \| undefined` → unchanged | `NONE < 65` is TRUE (lower discriminant), so NONE rows *survive*. Stripping the option would be wrong. |
| `type::table(f) = 'tbl'` | `Table` → `narrow_record_to` | `owner: RecordId<"user"> \| RecordId<"admin">` → `RecordId<"user">` | Every row satisfies the discriminant; `tbl` must be in the declared union. |
| `type::table(f) != 'tbl'` | `NotTable` → `narrow_record_without` | removes `tbl` from union | dual of above |

**Boolean algebra** (reuses `effects()` directly): `A AND B` contributes the **union** of both sides' positive effects (both hold on every row). `A OR B` contributes **nothing in P1**. P2 extends OR: a path `P` narrows only when *both* disjuncts narrow `P`, to the union of the two narrowed kinds.

```surql
-- P1
SELECT * FROM user WHERE email != NONE AND type::table(owner) = 'user'
--        email stripped of none  AND  owner collapsed to record<user>
-- P2
SELECT role FROM user WHERE role = 'admin' OR role = 'mod'
--        role: "admin" | "mod" | "guest"   →   role: "admin" | "mod"
```

**Scope + boundary gate.** WHERE-narrowing is disabled wholesale (return schema shape) under any of: no WHERE clause; `GROUP BY` present (result rows are groups, not source rows); non-plain-table FROM (subquery / param / graph source — no plain schema-object row to key against); VALUE projections (P1). An Effect is applied **only to a field returned under its own name**. A predicate on a non-projected field has no key to tighten (auto-restriction), and an aliased projection (`email AS e`) does not match by identity in P1 — both are no-ops. A sibling untouched by any guard keeps its schema kind (mirrors the `field_path_guard_does_not_narrow_a_sibling_path` test).

### 3.2 Permission-visibility (P1 plumbing, gated behavior)

**Ground truth** (`core doc/reduce.rs::reduce_document`): per surviving row, and only when `reduction_required` (non-owner / permission-enforced sessions), each field is reduced by its `FOR select`: `Full` → keep; `None` → `cut(k)` (field **removed**, not nulled); `Specific(e)` → bind `$value`/`$auth`/session, evaluate `e` against the row, `cut(k)` when not truthy. **Owner/root sessions skip reduction entirely** and see the raw schema shape.

**Defaults** (`core sql/permission.rs`): `Permission` defaults to `Full`; `DEFINE FIELD` with no clause → field select FULL (always present). `DEFINE TABLE` with no clause → table select NONE (a row-cardinality gate, **not** an element-type change).

**Classification** of a field's `FOR select`:

| `FOR select` | Class | Response rewrite |
|---|---|---|
| FULL / absent-default | ALWAYS | leave `field.kind` |
| NONE | NEVER | drop the entry (absent property) |
| `Specific(e)`, const-folds true | ALWAYS | leave |
| `Specific(e)`, always-false (7005) | NEVER | drop |
| `Specific(e)`, recognized dynamic (`$auth.id = id`, `$auth.role IN [...]`, row-field compare) | CONDITIONAL | `Kind::option(field.kind)` → `field?: T` |
| `Specific(e)`, opaque (`fn::` call, unmodeled access) | UNKNOWN | leave (never guess) |

```surql
DEFINE FIELD password ON user TYPE string PERMISSIONS FOR select WHERE $auth.id = id;
SELECT email, password AS pw FROM user;
-- { email: string; pw: string }   →   { email: string; pw?: string }
```

Explicit projections classify each field independently and carry the class to the aliased key. A projected computed expression (no single owning `FieldDef`) gets no rewrite. Nested subfield permissions (`DEFINE FIELD profile.email … FOR select …`) apply at that exact dotted path inside the nested object literal (P2 — path-addressed, mirrors `apply_omit`/`remove_kind_at_path`). Table-level `FOR select` never optionalizes or omits a field — it gates row presence (array cardinality), leaving the element type invariant.

**Perspective gate (soundness spine).** A new `AnalysisConfig` flag `auth_view = "owner" | "enforced"` (default `owner`):

- `owner` (default): **no permission narrowing** — schema shape, byte-for-byte today's behavior. The only sound default when we cannot prove the query runs under enforced permissions.
- `enforced`: apply the full classification.

The asymmetry: **CONDITIONAL → option is a widening** (safe under any perspective), but **NEVER → omit is a narrowing** (removes a field an owner would receive) and must never fire under the default. To avoid `| undefined` noise on owner-run queries, P1 emits CONDITIONAL optionalization only under `enforced` too; see open questions.

### 3.3 Flow / block narrowing (P1)

A block / function body / value-block / subquery block is typed by its **exit set**: the union over every reachable control-flow exit, each typed in the flow-narrowed env that must hold to reach it.

```
response_kind = Kind::either(exit_set)   // flattens, dedupes, collapses singletons; either([]) == Kind::None
```

Two exit kinds:

1. **`RETURN e`** — contributes the inferred kind of `e` in the env *after* the guard narrowings on the path to that RETURN (positive effects inside a THEN, fall-through effects after a diverging guard, FOR element-kind inside a loop). This is exactly where `narrow.rs::apply_effects` already mutates the env; inferring the RETURN expr in that env yields the narrowed kind for free.
2. **Trailing value** — the last statement's value, **only when control can reach the end of the block** (block does not provably diverge).

No-value diverging exits (`THROW`, `BREAK`, `CONTINUE`) contribute **nothing**.

**The current bug this fixes.** `analyze_block` (`flow/block.rs:13`) returns only `last` — the trailing statement's value. Non-trailing RETURNs are dropped. So
```surql
IF $x = NONE THEN RETURN false END; RETURN $x.name
```
is typed `string`, when the honest type is `bool | string`. That is a *soundness* bug in the response-kind path, not mere imprecision. `analyze_if_else` already unions branch kinds and applies per-branch effects (FB2 works inside a single IF), but an IF with no ELSE omits the implicit-NONE fall-through, and an enclosing block discards the early RETURN's value.

**Load-bearing invariant:** the exit set must **over-approximate** the runtime value set. Widening (adding an exit, adding `Kind::None`, falling to `Kind::Any`) is always sound; dropping a reachable exit is the failure mode. When exits cannot be enumerated, the whole block falls back to `Kind::Any`.

---

## 4. `$auth` model

**Ground truth** (`core dbs/session.rs:150,164`): `$auth = self.rd.map(...).unwrap_or(Value::None)` — the authenticated record, or NONE for root/NS/DB/JWT-without-subject sessions. So the honest type is `option<record<…>>`. `$session.rd` mirrors `$auth`; `$access` is a string; `$token` / `$session` are objects. A `DEFINE ACCESS … TYPE RECORD` does not fix a table — its target is whatever the SIGNUP / SIGNIN / AUTHENTICATE subquery returns (`core sql/access_type.rs:284`).

### Phase 1 — seed `$auth` as a narrowable record fact

Today `$auth` is bound only inside permission-predicate analysis (`permissions.rs:96`, `Kind::Record(vec![])`) and hover; at the top level of a query the env is `StatementEnv::default()`, so `$auth` is an *unbound* param — `infer.rs:31` misclassifies it as a host `record_param_use` typed Unresolved. That is a latent bug: `$auth` is engine-supplied, never host-supplied.

P1 seeds all session/access params as **bound facts** in every analysis env (top-level query, `fn::` body, permission predicate):

```
$auth: option<record>   // Kind::Either([Kind::None, Kind::Record([])])
$token: object   $session: object   $access: string   $scope: string
```

Because they are bound, `infer.rs` stops reporting them as host params, and `narrow.rs` can refine them. An open record (empty table list) is the sound top for "a record of unknown table" — member access degrades to partial/Any (`field_of_kind` returns None on empty Record targets), raising **no false 1002**. Including NONE is required by core semantics.

**Narrowing guards that now work for free (P1):**

```surql
IF $auth != NONE THEN SELECT * FROM post WHERE owner = $auth END
--  $auth : option<record>  →  record   (existing narrow_out_none path)

IF type::table($auth) = 'user' THEN fn::user_only($auth) END
--  $auth : option<record>  →  record<user>   (positive branch drops NONE + non-user tables)
```

**One new recognizer** (`narrow.rs`, mirroring `type_table_path`): `type::is_record($auth, 'user')` → `record<user>` on the **positive branch only** (the negative branch narrows nothing — `false` is satisfied by NONE, a non-record scalar, or another table). Bare `type::is_record($auth)` → non-none open `record` (strip NONE only). Lowering normalizes `type::is::record` → `type::is_record` (`mod.rs:55`).

### Phase 2 — DEFINE ACCESS binding

Model each record-access method's target table(s), resolved from the response kind of its AUTHENTICATE subquery (precedence: AUTHENTICATE > SIGNIN > SIGNUP, since AUTHENTICATE can rewrite `$auth` to any record). Store an `AccessDef { name, level, record_tables }` in `SchemaIndex`. Seed `$auth` as `option<record<union of all DB-level record-access targets>>`:

```surql
DEFINE ACCESS user_access ON DATABASE TYPE RECORD
  SIGNIN (SELECT * FROM user WHERE name = $name)
  SIGNUP (CREATE user SET name = $name);
--  $auth : option<record<user>>
```

The **union** (not any single table) plus NONE is the only sound default: a single session holds one method, but the generator cannot know which, and system/JWT sessions carry NONE. Any method whose target does not resolve to a concrete `record<T>` (dynamic `type::table($t)`, computed target) collapses that method's contribution to open `record<>`, degrading the union to `option<record>` — never a wrong single table. Only DATABASE-level record access populates `$auth` for DB sessions.

### Phase 3 — permission-gated option fields via scoped session

A project **declares** (config/pragma) which access method a query set runs under and whether it is authenticated; `generate` then specializes `$auth` to the single non-optional `record<user>` of that method, so `$auth`-dependent permission gates evaluate precisely:

```surql
// surrealql-analyzer config: { "session": { "access": "user_access", "authenticated": true } }
SELECT *, password FROM user;
--  $auth : record<user>   (only under an explicit scope declaration)
```

This is a **declared assumption**, never proven from the query text — sound only within the declared scope. It must be opt-in: the analyzer must never default to "authenticated", because assuming non-none `$auth` would wrongly promote permission-gated optional fields to required. No scope declared → keep the P2 `option<record<union>>`, and permission-gated fields stay `option<…>`. A declared access name matching no `AccessDef` → diagnostic + fall back to P2.

**P3 cross-narrowing (advanced).** When a query's WHERE provably entails a field's select gate — `SELECT * FROM ONLY user WHERE id = $auth.id` against `FOR select WHERE $auth.id = id` — the query provably returns only the caller's own row, so `password` recovers to present `T`. Recovering-to-present is the *only* unsound direction; it demands the strictest proof (WHERE pins `id` to `$auth.id`, field predicate is exactly the ownership shape, `$auth` modeled as a concrete record) and defaults off. Any gap keeps `option<T>`.

---

## 5. Blocks, functions, and subqueries

The flow facet (§3.3) is the general mechanism; here is how each construct routes into it.

**RETURN sink.** Add a RETURN-sink frame stack to `AnalysisContext`, pushed at function/closure/value-block boundaries and popped returning the accumulated `Vec<Kind>` (mirror `with_loop` / `with_child_env`). `analyze_return` infers the value in the already-narrowed env and pushes the kind into the top frame (still returning it for the diverging-as-last-statement case). `analyze_block` becomes an exit-set builder: `Kind::either(return_sink ∪ (trailing_value if !block_diverges))`, distinguishing RETURN-divergence (counted) from THROW/BREAK/CONTINUE-divergence (dropped).

**IF without ELSE (FB3).** When `else_branch` is None and not all branches diverge, add `Kind::None` to `branch_kinds` — the implicit fall-through evaluates to NONE at runtime (verify against `IfelseStatement::compute`). Positive/negative effect application is the existing FB2 hook, unchanged.

**FOR loops (FB5).** Run the body under the sink so inner RETURNs bubble; keep returning `Kind::None` and keep FOR non-diverging.

**Value blocks / pure path (FB6).** `pure_block_kind` and `statement_value_kind` (`expression/infer.rs`) converge on the sink-based `analyze_block` (today `pure_block_kind` returns on the first RETURN and treats IfElse/For as None). `expr_fact` already routes value-blocks to `analyze_block`, so it inherits union semantics.

**Function return exposure (FB7).** `analyze_define_function` body_kind becomes the exit-set union, fed into 2012 assignability and, when `return_ty` is None, into `FunctionDef.return_kind` (consumed at custom-call resolution, `function/mod.rs:89`). Gate FB7 on non-recursive bodies to avoid infinite inference on 5009 cycles.

**Subquery FROM sources.** In P1 a subquery/param/graph FROM disables WHERE-narrowing (§3.1 boundary gate) — there is no plain schema-object row to key against; the source returns its own (possibly flow-narrowed) kind.

---

## 6. Pipeline threading and reaching `generate` / hover

The narrowed kind stays the single `response_kind` channel end-to-end. Exact hooks:

**WHERE post-pass — `crates/workspace/src/analyzer/data/select.rs`.**
Add `fn apply_where_narrowing(row_kind: Kind, stmt, table, ctx) -> Kind`, called in `select_response_kind` immediately after `row_kind` is built and **before** `apply_omit` (~line 76). It returns `row_kind` unchanged unless `stmt.where_clause` is Some, `stmt.group` is None, and FROM resolved to a plain schema table (the `resolve_from_table` Ok path). For recognized cases it collects `flow::narrow::where_effects(cond, ctx.env())` and, for each Effect whose `GuardRoot::Row` segment path maps to a projected key, walks the segments into the `Kind::Literal(Object)` map (same recursion shape as `insert_kind_at_path` / `scalarize_at_path`, but **tighten-only**: skip absent keys) and replaces the leaf with `narrow_kind(current, &effect.narrowing)` when it tightens. Runs before `apply_omit` / `apply_fetch` / `apply_split`, which already operate on the same object-literal shape.

**Flow-engine extraction — `crates/workspace/src/analyzer/flow/narrow.rs`.**
1. Generalize `GuardPath` to carry a root: `root: GuardRoot` where `enum GuardRoot { Param(String), Row }`. `bare(param)`/`is_bare()`/`key()` keep Param semantics (existing callers unchanged); add `row(fields: Vec<String>)`. `key()` for a Row root uses a reserved prefix (`"@row."`) so it can never collide with a param `narrowed_paths` key.
2. Add `pub(crate) fn where_effects(cond, env) -> Vec<Effect>` calling the existing `effects(cond, /*positive*/ true, env)` recursion, threading a `row_ok: bool` flag into `leaf_effect` / `guard_path_of` so bare-field idioms (via `plain_field_segments`, an `IdiomPart::Field`-rooted chain) resolve to a `GuardRoot::Row` path; param extraction unchanged.
3. Extend `Narrowing` with `StripNone` (strip `none` only) and `Eq(Kind)` (literal singleton), plus arms in `narrow_kind` (`StripNone` → `narrow_out_none_only` filtering only `Kind::None`; `Eq(k)` → `Some(k)` when `k`'s base unifies with the current non-none base, else `None`). Keep `NotNone` = strip none+null for `>`/`>=` and legacy param paths.
4. Extend `leaf_effect`: literal-equality → `Eq`; `>`/`>=` → `NotNone`; `<`/`<=` → no effect; split the NONE arm so `!= NONE`/`IS NOT NONE` yields `StripNone`; add a `!= NULL`/`IS NOT NULL` arm yielding a null-only strip.

**Permission plumbing (P1).**
- AST — `crates/syntax/src/ast/statement.rs`: replace the lossy flat `permissions: Vec<Spanned<Expr>>` on `DefineField` / `DefineTable` with a structured `SelectPermission` (`Full | None | Specific(Spanned<Expr>)`), retaining the flat list for the existing predicate walk (back-compat with `query.rs`).
- Lowering — `crates/syntax/src/lower/statement.rs::lower_permission_predicates` (line 318) currently drops the `FOR <action>` keyword and NONE/FULL choice; the grammar already carries them (`grammar.js PermissionGroup:1434`). Add `lower_permissions` reading the action token(s) and the NONE/FULL/WhereClause arm.
- Schema — `crates/workspace/src/schema.rs`: add `select_permission: SelectVisibility` (serde enum `Always | Never | Conditional | Unknown`) to `FieldDef` and `TableDef`, populated in `field_def_from_ast` / `table_def_from_ast` via a syntactic classifier over the structured permission AST. Store the **pre-classified enum**, not an Expr (SchemaIndex is serde-serialized). Reuse the 7005 always-false family (`analyzer/expression/check.rs::disjoint_records`) for Never, `infer_expression_fact(...).value == Value::Bool(_)` for Always, and `permissions.rs::bind_row_params` ($auth = Record([])) for the dynamic-gate recognizer.
- Application — in `select.rs`, apply the per-field visibility rewrite in `object_kind_for_all_fields` / `object_kind_for_field_prefix` (wildcard), `project_expr` (explicit/aliased), and `value_projection_kind` (VALUE): `Kind::option(...)` for Conditional, drop the entry for Never (mirror `remove_kind_at_path`). **Do NOT change `kind_for_path`** — it is shared with WHERE/aggregate *checking* contexts that must keep the schema shape.

**Perspective + policy config — `crates/workspace/src/config.rs`.**
Add to `AnalysisConfig`: `auth_view` (`owner`|`enforced`, default `owner`), `narrow_response_kind` (bool, default `true`), `narrow_permissions` (bool, default `true`). Thread a `NarrowingPolicy` from `WorkspaceConfig` (already available at `analysis.rs:212`) into `AnalysisContext` (`context.rs`) so `select.rs` can gate behavior. `narrow_response_kind = false` skips the entire post-pass (provably identical to today, since narrowing is a pure post-pass). `narrow_permissions = false` keeps P1 WHERE-narrowing but disables P2/P3.

**`$auth` seeding — `crates/workspace/src/analyzer/`.**
Add `session_context_params() -> BTreeMap<String, Kind>` (single source of truth), seeded into the top-level env at `pipeline.rs:145` and into `fn::` body envs, replacing the duplicated `permissions.rs::bind_row_params` session block and `context_params.rs::insert_session_params`. Change the `$auth` kind at both sites from `Kind::Record(vec![])` to `Kind::option(Kind::Record(vec![]))`. Add `token`/`session`/`access`/`scope` to `CONTEXT_ONLY_PARAMS` (`expression/mod.rs:85`) so siblings are not reported as host params.

**Generation — no codegen change needed.**
`crates/codegen/src/lib.rs::strip_none` (line 94) makes `Either[None, T]` the **sole** producer of an optional `?` property and drops the `None` variant, and omits absent object entries. So both directions map for free:
- WHERE-narrowed `option<string>` → `string`: `email?: string` → `email: string`.
- Permission-widened `string` → `option<string>`: `password: string` → `password?: string`.
- `Kind::Literal("active")` renders as the TS literal type `"active"`.

Convention (`lib.rs:11`): `NONE` → `undefined` (optional prop); there is no `| null`. Keep it — a dedicated `Kind::Null` already renders `null` if a future rule needs it. `crates/cli/src/main.rs::run_generate` (line 285) → `.response_kind` → `ts_type` (line 335) picks up the narrowed kind unchanged; add `--auth-view` to surface the config. The Rust adapter (`crates/rs/`) consumes the same `response_kind` and needs no result-type change.

**Interaction ordering.** When a field is both WHERE-narrowed (remove NONE) and permission-widened (add NONE) — e.g. `SELECT password FROM user WHERE password != NONE` on a gated `password` — **permission widening dominates and applies after WHERE narrowing**: the row can satisfy WHERE yet still have `password` cut for *this* caller. So run WHERE tighten, then permission widen.

**Hover / inlay — `crates/workspace/src/query.rs`.**
Because the narrowed kind *is* `response_kind`, `hover_at` (line 96) and inlay labels (line 63) render it through `render_kind` for free. **Add provenance:** when a leaf differs from its schema `FieldDef.kind`, append a suffix — `email: string  (narrowed from option<string> by WHERE email != NONE)` / `password?: string  (gated by PERMISSIONS FOR select)`. This is the user's only in-editor signal that a type is refined and is essential for trust (it makes an over-narrowing bug visible). Provenance rides a small editor-only `Vec<(path, schema_kind, narrowed_kind, reason)>` on `StatementAnalysis`, populated only when the two kinds differ — a real, immediate reader exists (hover), consistent with `let_bindings`. It never feeds a diagnostic.

---

## 7. Phased rollout

### P1 — foundational + WHERE + flow + `$auth` seed

Scope: the narrowing post-pass hook, `NarrowingPolicy` opt-out, hover provenance; WHERE non-NONE / null / literal-eq / ordering / discriminant narrowing (reusing `narrow.rs`); the block exit-set fix (RETURN sink, IF-no-ELSE NONE branch, THROW/BREAK/CONTINUE drop); `$auth` seeded as `option<record>` with `type::is_record` recognizer. Permission *plumbing* (AST/lowering/schema `SelectVisibility` + `auth_view` flag) lands here too, but permission *narrowing behavior* is inert under the default `owner` perspective.

Acceptance criteria:
- `SELECT name, email FROM user WHERE email != NONE` generates `Array<{ name: string; email: string }>`.
- `SELECT status FROM order WHERE status = 'active'` generates `status: "active"`.
- `SELECT age FROM user WHERE age < 65` is **unchanged** (`age: number | undefined`) — the MUST-NOT case.
- `IF $x = NONE THEN RETURN false END; RETURN $x.name` types as `bool | string`.
- `SELECT * FROM user WHERE email != NONE GROUP BY country` is unchanged (GROUP disables narrowing).
- `narrow_response_kind = false` produces byte-for-byte today's output on the entire corpus.
- `$auth` no longer reported as a host param; `owner` perspective leaves every result type at schema shape.

### P2 — permission option-fields

Scope: turn on classification under `auth_view = enforced` — ALWAYS/NEVER/CONDITIONAL(recognized)/UNKNOWN over wildcard + explicit + VALUE projections; const-fold always-true/false (reuse 7005); nested/object subfield permissions (path-addressed, mirror `apply_omit`); DEFINE ACCESS → `AccessDef` → `$auth: option<record<union>>`; WHERE-narrowing alias threading (source-path → projection-key map) and OR narrowing (both-disjuncts rule).

Acceptance criteria:
- Under `enforced`: `SELECT email, password FROM user` with `password` gated → `{ email: string; password?: string }`.
- `FOR select NONE` field omitted under `enforced`, present under `owner`.
- `FOR select WHERE false` omitted; `WHERE true` present.
- `fn::can_read($auth)`-gated field stays present (UNKNOWN).
- `SELECT email AS e FROM user WHERE email != NONE` narrows `e`.
- `$auth: option<record<user>>` given a single DB-level RECORD access.

### P3 — `$auth` recovery + cross-narrowing

Scope: declared session scope specializing `$auth` to non-optional `record<T>`; WHERE-provably-satisfies-gate recovery (`SELECT * FROM ONLY user WHERE id = $auth.id` → `password: string`); FETCH-materialized record fields inheriting the target table's field visibility; table-level Specific surfaced as a non-type row-cardinality diagnostic; `NOT (pred)` handling.

Acceptance criteria:
- With a declared authenticated scope, `SELECT *, password FROM user` recovers `password: string`.
- Without a scope, the same query keeps `password?: string`.
- A declared access name matching no `AccessDef` emits a diagnostic and falls back to P2.
- Recovery never fires without the full ownership proof.

### Test strategy — the workshop oracle

The acceptance oracle is the tsc-proven contract from MEMORY (`project_ts_runtime_design`): for each narrowed query, the generated `.d.ts` `result_type` must **tsc-typecheck against the real SurrealDB runtime response**. Concretely:

1. **Golden `Kind` unit tests** in `select.rs` / `narrow.rs`: assert each rule's before/after `Kind` (the tables in §3), including every MUST-NOT case (`<`/`<=` no-op, sibling-untouched, OR-empty in P1, table-level-permission element invariance).
2. **Round-trip codegen tests**: assert `ts_type` renders the narrowed `Kind` to the expected TS (never string-patch TS; verify `strip_none`'s `[]`/`[only]`/`[_,_]` collapse loses no non-None variant).
3. **Workshop corpus** (the oracle): drive `generate` over the real corpus of `WHERE != NONE` and `FOR select WHERE $auth.id = id` cases; each emitted `result_type` must tsc-typecheck against the recorded SurrealDB response. Where the corpus lacks coverage, synthesize fixtures against a live `surrealdb` instance and record its reduced responses under both owner and enforced sessions.
4. **Opt-out invariant test**: `narrow_response_kind = false` diffs empty against the pre-narrowing baseline across the whole corpus.
5. **Regression guard**: the existing `permissions.rs` F0–F4 suite (E1002/E5001/E7005/E2005) must stay green after the `$auth: option<record>` change — comparisons like `owner = $auth` now compare `record<user>` against `option<record>`.

---

## 8. Prior art, open questions, risks

### Prior art (short)

- **TS control-flow analysis / occurrence typing** (Tobin-Hochstadt & Felleisen's Typed Racket): a guard refines a variable's type in the branch where it holds; discriminated unions narrow on a literal tag. This is exactly `flow/narrow.rs` today; a SELECT's WHERE is the guard whose "then-branch" is the retained row set. TS narrowing is unsound under aliasing/mutation — but a SELECT row is immutable within the query, so it does not bite.
- **Refinement / Liquid types** (Flux-for-Rust, Liquid Haskell, F*): base types carry logical predicates; subtyping is implication. We borrow the *representability gate* (apply a refinement only if it maps to a tightening `Kind`) but explicitly **do not** build an SMT solver — we restrict to the decidable syntactic guard subset `narrow.rs` recognizes.
- **SQL row-level security + outer-join/view nullability** (Postgres RLS `USING`, column GRANTs; LEFT-JOIN columns becoming nullable): the exact analogue for field permissions. A non-total `FOR select` is a per-row column grant, so the field widens to `option<T>` — soundness is trivial (widening is a superset), so we never evaluate `$auth`.
- **EdgeDB / Prisma / PRQL result typing**: EdgeDB shapes yield precise closed objects with per-link cardinality; `FILTER exists .email` narrows to required. `projected_object_kind` already is shape-typing. Pitfall: EdgeDB is closed-world; SurrealQL has schemaless tables and dynamic idioms — keep the `Kind::Any` fallback and never narrow a computed/dynamic projection.
- **Datalog / relational refinement** (selection pushdown, equality propagation): the soundness proof that WHERE-narrowing is safe over a *set* — the predicate holds of every surviving tuple. This justifies WHERE narrowing but explicitly does **not** justify permission narrowing (per-row/session, not set-uniform), which is why permissions must widen, not narrow.

### Open questions

1. **`auth_view` default**: `owner` (safe, facet inert — proposed) vs. `enforced`-by-default for schemas that declare any `FOR select` predicate (product framing: "flagship $auth feature"). Recommend owner-default for soundness.
2. **CONDITIONAL under owner**: option is a safe widening even for owners, so should recognized `$auth` gates emit `option<T>` regardless of perspective (precision) while NONE-omission stays enforced-only? Proposed enforced-only, to avoid `| undefined` noise on owner-run queries — needs a product call.
3. **Legacy `NotNone` split**: should the param-guard `NotNone` (IF `!= NONE` guards, discriminant fall-through) also become `StripNone` to fix the latent null-stripping unsoundness for params typed with an explicit `null` variant? Changing it touches flow/if_else/block tests; `option<T>` (no null) may be universal enough there to defer.
4. **VALUE projections**: `SELECT VALUE email FROM user WHERE email != NONE` should scalarize to `string` — needs the VALUE expr's field path matched against Effects (P2).
5. **Membership `f IN ['a','b']` / `f INSIDE 18..65`**: does core evaluate `NONE IN [...]` as false (implying NotNone), and can we soundly emit a literal-union `Eq` for arrays of literals? Needs a core `contains`/`inside` semantics check first.
6. **Set-vs-row permission semantics**: confirm in `/Users/drewridley/Documents/Projects/surrealdb` that field-level `FOR select` deny **cuts the field** (making it `option<T>`) rather than dropping the whole row — the entire permission facet hinges on this (`doc/reduce.rs::reduce_document` says cut; verify).
7. **IF-no-ELSE runtime value**: confirm SurrealDB 3.0 evaluates a no-match `IF` with no `ELSE` to `NONE` (against `IfelseStatement::compute`) — the FB3 NONE branch is sound only if so.
8. **Sink vs. `BlockValue` struct**: is a ctx-carried `Vec<Kind>` sink stack cleaner than `analyze_block` returning `BlockValue { returns, trailing, diverges }`? The struct is more local but forces every caller to fold; the sink centralizes but adds mutable ctx state (the reader is real and immediate, so it clears the `no_speculative_state_channels` bar).
9. **P3 declaration surface**: where does the session scope live — per-file pragma, per-query annotation, or global generate config — and how does it compose with the WHERE and permission facets' generate inputs?
10. **AUTHENTICATE precedence** and **union scope**: confirm AUTHENTICATE > SIGNIN > SIGNUP for target resolution, and that `$auth` unions only same-level (DATABASE) record-access targets plus NONE.

### Risks

- **Over-narrowing = wrong types.** The dominant risk. Mitigated structurally by the prove-or-fallback rule (`Option<Kind>` tightening only), the reuse of already-proven `narrow.rs` transforms, the MUST-NOT cases, and hover provenance surfacing any leaf that differs from schema. The opt-out (`narrow_response_kind = false`) is a provably-identical escape hatch.
- **Unsound permission omission.** NEVER → omit removes a field owners receive; gated behind `auth_view = enforced` and never fired on the default perspective.
- **`$auth` misclassification churn.** Seeding `$auth` as `option<record>` changes the type of `owner = $auth`-style comparisons in the permission test suite (F0–F4); must stay green.
- **AST/lowering churn for permissions.** Replacing the flat action-untagged permission `Vec` touches `query.rs` back-compat consumers; keep the flat list alongside the structured `SelectPermission`.
- **Corpus coverage.** The oracle needs enough real `WHERE != NONE` and `FOR select WHERE $auth.id = id` cases; synthesize fixtures against a live instance where the corpus is thin.