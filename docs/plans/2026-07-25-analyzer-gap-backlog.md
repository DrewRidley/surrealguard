# Analyzer Gap Audit — 56 confirmed gaps → 34 work items

_Produced by an 8-lens multi-agent audit (2026-07-25). Every gap was reproduced against the
prebuilt analyzer and adversarially verified. Hand-spot-checked afterwards: TI-1 (`??` E2004 FP)
and DX-1 (GROUP BY alias E1002 FP) both reproduce exactly._

**Additional finding not in the audit (verified by hand):** `surrealguard check` scans only
`.surql` files — it never reads host files (`.ts/.tsx/.svelte/...`), so embedded queries are
invisible to it. A project whose `generate` aborts with 2 errors passes `check` green with
exit 0. CI gating on `check` does not cover the embedded-query surface at all.

---

# SurrealGuard Analysis-Gap Backlog

**Source:** 56 independently reproduced-and-confirmed gaps, deduped to **34 work items**. Several lenses hit the same root cause from different angles (the implicit-`id` cluster alone accounted for 5 separate filings); those are merged with their symptom lists intact.

**Two methodology facts worth encoding in the repo docs**, because ~15 verifiers independently rediscovered them and one wasted a full cycle on it:

1. `surrealguard generate` only emits registry entries for queries **embedded in host files** (`.ts/.tsx/.js/.jsx/.svelte/.vue/.astro` via `surql`/`db.query`). A `queries/**/*.surql`-only project is *checked* but produces an **empty `SurqlRegistry`**. All repros below assume a host `.ts` probe file.
2. `/Users/drewridley/Documents/Projects/workshop/database` has **no host files**, so `generate` over it is empty. Corpus type-impact must be probed by copying `schema/` into a scratch project with a probe `.ts`.

---

## Top 5 highest-leverage fixes

Ranked by (workshop sites × soundness) ÷ effort. All five are sound, none are speculative, and all have the "answer is already computed elsewhere in the codebase" property.

| # | Item | Why it's top | Effort |
|---|------|--------------|--------|
| 1 | **[TG-1] Wildcard/mutation rows omit implicit `id` / `in` / `out`** | Hits 100% of `SELECT *`, every `CREATE/UPDATE/UPSERT/INSERT/RELATE/DELETE` return row, every FETCH materialization, and all 20 corpus RELATION tables. Corpus declares **zero** explicit `id` fields, so *every* row type in the corpus is missing its primary key. `implicit_field_kind` already computes the right answer and the explicit-projection path already uses it. | S–M (code small; test-expectation fallout dominates) |
| 2 | **[TI-1] `??` does not strip NONE from its LHS** | Not merely loose — produces an **error-severity E2004 false positive** on `(optfield ?? 'd') + '!'`, which aborts `generate` for the whole workspace. Three purpose-built arms (`?? []`, `?? 0`, `?? NONE`) are all defeated. Fix is one `let lhs = &narrow_out_none(lhs);` using a helper 180 lines above the bug. | **S** |
| 3 | **[DX-1] `GROUP BY <alias>` / `ORDER BY <alias>` false-positive E1002** | Error severity → registry not written. Valid in SurrealDB 3.0.5 (verified against a live engine). The alias-aware set is *already built* one function over for W4013/2017 and simply isn't consulted by the earlier field-path check. | **S** |
| 4 | **[SX-1] Descendant field definitions clobber the parent's declared kind** | Causes **error-severity E2001 on valid writes** to `array<object>` + `[*]` and `option<object>` + subfields — two of the most common real schema shapes. Reproduces on the all-valid oracle corpus (`organization_billing.setup_checkout_line_items` emits a wrong type today). Also swallows the never-emitted 1025. | M |
| 5 | **[TI-2] Record-link traversal only sees bare `record<T>`** | **110** `option<record<…>>` + 29 `array<record<…>>` declarations in the corpus — the single most frequent link shape. Every traversal through one silently emits `unknown`. The equivalent unwrap already exists for `option<object>` and `array<object>` sub-field declarations, so there's in-tree precedent for both the unwrap and the flatten convention. | M |

**Runner-up worth doing first anyway (10-minute fix):** [TI-3] — a single mismatched predicate at `pipeline.rs:317` that leaves three real corpus fields as `unknown | number`.

---

## Cross-cutting: error-severity false positives that block `generate`

This is the most damaging class in the whole audit. Each of these is a **hard error on valid SurrealQL**, and because `generate` refuses to write the registry when any embedded query errors, **one occurrence anywhere kills codegen for the entire project**. The all-valid workshop oracle only stays green because it happens to avoid these constructs — the zero-FP corpus is *not* protecting this surface.

| Item | Trigger | Severity |
|---|---|---|
| TI-1 | `(optional ?? 0) + 1` | E2004 |
| DX-1 | `SELECT x AS n FROM t GROUP BY n` / `ORDER BY n` | E1002 |
| SX-1 | `CREATE t SET items = [{…}]` where `items[*]` is declared | E2001 |
| DX-2 | `SELECT math::sum(price * qty) … GROUP ALL`, `math::sum(a)+math::sum(b)` | E5002 |
| DX-3 | `CREATE t CONTENT $payload` (and `CONTENT $letBoundObject`) | E2034 ×N |
| TI-4 | `CREATE t SET st = 'active'` where `st` is `TYPE 'active' \| 'inactive'` | E2001 |
| GR-1 | `SELECT <~post AS b FROM person` (reference back-link in a query) | E3001 |
| GR-2 | `SELECT ->(likes, wrote)->post FROM person` | E3001 |
| SY-1 | `FOR $t IN $u.tags { … }`, `FOR $x IN fn(…) { … }` | S0001 |
| SY-2 | `RETURN not(true)`, `RETURN sleep(1s)`, `DEFINE SEQUENCE …` | S0001 |
| FN-1 | `vector::distance::euclidean(…)`, `array::sort::asc(…)`, `rand()` (~50 names) | E5001 |
| TI-5 | `LET $a = [[1,2],[3]]; RETURN $a[0][0];` | E2030 / E5001 |
| FN-2 | `rand::duration(1s, 2s)` | E5002 |

Recommend triaging this table as a single "FP wave" milestone ahead of the inference-precision work — a false error is strictly worse than a loose type.

---

## A. Type inference

### TI-1 — `??` (coalesce) does not strip NONE from its left operand
**Merged from:** 1 filing. **Effort: small.**

**Root cause.** `binary_result_kind`'s every `Op::NullCoalesce` arm pattern-matches the *bare* LHS kind (`matches!(lhs, Kind::None | Kind::Null)`). An `option<T>` is `Kind::Either([None, T])`, matches no arm, and falls to the catch-all `Kind::either(vec![lhs, rhs])` — which re-unions the NONE back in. The arms were written for a literal `NONE`, never for an option.

**Repro.**
```surql
DEFINE FIELD nick ON t TYPE option<string>;
SELECT VALUE nick ?? 'd' FROM t;
SELECT VALUE (nick ?? 'd') + '!' FROM t;
```
**Current:** `Array<undefined | string>`; the second raises `error[E2004]: '+' can't combine a 'option<string>' and a 'string'` with a help that tells the user to `?? <default>` — which they did. `generate` aborts.
**Expected:** `Array<string>`; no diagnostic. Also fixes `?? []` (currently leaks `Array<unknown>` in) and `?? 0` (currently `number | int`).

**Fix.** Bind `let lhs = &narrow_out_none(lhs);` at the top of the NullCoalesce group so all existing arms see the stripped kind. `narrow_out_none` already exists in the same file and is a no-op on `Kind::Any` and on unions that would empty out, so `Any ?? x` and `NONE ?? x` stay correct.

**Touch:** `crates/workspace/src/analyzer/expression/infer.rs:933-956`, helper at `:702`.

> Note: the previously-cited corpus hit (`fn::organization::employee`) is **confounded** — its `undefined` comes from two `IF … RETURN NONE END` guards, not the coalesce. Don't cite it.

---

### TI-2 — Record-link traversal only sees bare `record<T>`; `option`/`array`/`set` links degrade to `unknown`
**Merged from:** 1 filing (+ overlaps TG-4 FETCH). **Effort: medium.**

**Root cause.** `record_link_targets_at` returns `match &field.kind { Some(Kind::Record(t)) => Some(t), _ => None }`, and `kind_for_path`'s head match likewise accepts only `Kind::Record(_)`. Anything wrapped in `Option`/`Array`/`Set`/`Either` falls to `Kind::Any`.

**Repro.**
```surql
DEFINE FIELD owner   ON team TYPE record<user>;          -- control
DEFINE FIELD lead    ON team TYPE option<record<user>>;
DEFINE FIELD members ON team TYPE array<record<user>>;
SELECT owner.name AS o, lead.name AS ln, members.name AS x FROM team;
```
**Current:** `{ o: string; ln: unknown; x: unknown }`. `SELECT lead.bogus` also emits no E1002 (suppressed by `field_is_opaque_boundary`).
**Expected:** `ln: option<string>`, `x: Array<string>` (or flattened, matching the existing `array<object>` + `.*` convention), `members.{name}` → `{ name: string }`.

**Corpus:** 110 `option<record<`, 29 `array<record<` declarations. `SELECT parent.name, sectors.name FROM organization` → both `unknown` today.

**Fix scope (why it's medium, not a match widening):** `record_link_targets_at` must return a wrapper descriptor, not bare targets; `resolve_across_link` must re-wrap; the parallel destructure call site needs the same; `kind_for_path`'s head match must widen in tandem; and `field_is_opaque_boundary`/`validate_field_path` must be updated together, since newly-traversable links will start emitting E1002 — needs a corpus FP pass.

**Touch:** `crates/workspace/src/analyzer/data/select.rs:1818, 1800, 1755, 1880, 1449, 1900`.

---

### TI-3 — Untyped COMPUTED/VALUE fields with one catalog-independent arm keep a stale `Any | T`
**Merged from:** 1 filing. **Effort: small (one predicate).**

**Root cause — *not* what the symptom suggests.** This has nothing to do with `IF` guards or `$this` narrowing (all of those were disproven experimentally). `pipeline.rs:310-322` re-installs the globally-inferred field kind only `if field.kind.is_none()`. For a fully catalog-dependent COMPUTED the per-source pass yields pure `Kind::Any`, which maps to `None`, so the reuse fires. But when *one* arm is catalog-independent (a literal, `NONE`, an implicit fall-through), the per-source pass yields `Either([Any, Number])` — non-`None` — so the correct, fully-resolved global kind is refused.

**Repro.**
```surql
DEFINE FIELD b1 ON m COMPUTED IF $this.qty > 0 THEN $this.spend ELSE 0 END;
```
**Current:** `b1: unknown | number`. (`… ELSE $this.spend END` correctly gives `number` — proving guards, `$this`, sibling reads and `!= NONE` narrowing all work.)
**Expected:** `number`.

**Fix.** Mirror PRE-PASS 1c's predicate: `if field.kind.as_ref().map_or(true, crate::schema::kind_contains_any)`. PRE-PASS 1c's comment already describes this exact `Any | T` case. Sound because `global_field_kinds` only ever holds non-`Any` kinds inferred against the full catalog for *untyped* fields, so this strictly upgrades.

**Corpus:** `customer.average_order_value` → `unknown | number`; `cash_drawer_session.variance`, `inventory_movement.total_cost`, `inventory_transaction.total_cost` → `?: unknown`.

**Touch:** `crates/workspace/src/analyzer/pipeline.rs:317` (compare `:168-211`).

---

### TI-4 — String/number literals are not narrowed in assignment position → E2001 FP on literal-union fields
**Merged from:** discovered while verifying SX-6. **Effort: small–medium. Blocking prerequisite for SX-6.**

**Root cause.** `check_assignment_value` uses `infer_expression_fact(...).kind`, which is `Kind::String` for a string literal; `kind_is_assignable_to(String, Either([Literal…]))` is false.

**Repro.**
```surql
DEFINE FIELD st ON t TYPE 'active' | 'inactive';
CREATE t SET st = 'active';                 -- error[E2001]
CREATE t CONTENT { st: 'active' };          -- error[E2001]
UPDATE t SET st = 'inactive';               -- error[E2001]
```
**Current:** three false errors. **Expected:** clean.

**Why it's urgent:** the corpus dodges this only because it uses `string` + `ASSERT` instead of literal unions. The moment SX-6 (ASSERT→literal-union narrowing) lands, all 118 assert-enum fields route through this path and every valid write in the corpus becomes an error.

**Touch:** `crates/workspace/src/analyzer/data/mutation.rs:520` (and the CONTENT/INSERT equivalents), `crates/workspace/src/kinds.rs::kind_is_assignable_to`.

---

### TI-5 — `Kind::Either` is not distributed over element/index/method operations
**Merged from:** 1 filing (FOR-loop lens). **Effort: medium.**

**Root cause.** Three sites test collection-ness with a flat `matches!(k, Kind::Array(..) | Kind::Set(..))` and never map over an `Either`'s arms:
- `for_loop.rs:29-32` `element_kind` → loop var becomes `any`, which then absorbs the enclosing block's entire exit union.
- `check.rs:305-315` (E2030 index/filter contract).
- `check.rs` method resolution (E5001).

**Repro.**
```surql
RETURN { LET $a = [[1,2],[3]]; RETURN $a[0][0]; };   -- error[E2030] "array<int,2> | array<int,1> can't be indexed"
RETURN { LET $a = [[1,2],[3]]; RETURN $a[0].len(); };-- error[E5001] no method `len`
RETURN { LET $a = [[1,2],[3]]; FOR $v IN $a { … }; RETURN 'z'; }; -- [unknown]
```
**Current:** two hard errors on valid SurrealQL (aborting `generate`) plus total type loss in the FOR case.
**Expected:** all arms are arrays ⇒ no finding, union the element kinds.

**Note.** `query.rs:1667` already distributes over `Either` — but with `find_map` (first arm wins), which is itself wrong for `array<int> | array<string>`. The real fix is one shared helper that **unions** every collection arm, consumed by all four sites. Policy rule to preserve: *all* arms collections ⇒ no finding; *some* arm definitely non-collection (e.g. `option<array<T>>`'s NONE) ⇒ keep the E2022/E2030 finding but still union the collection arms so inference doesn't collapse.

**Touch:** `crates/workspace/src/analyzer/flow/for_loop.rs:29-32`, `crates/workspace/src/analyzer/expression/check.rs:305-315`, `crates/workspace/src/query.rs:1667`.

---

### TI-6 — Casts to any parameterized/compound target infer `unknown`
**Merged from:** 2 filings (typegen-inventory + expr-builtins — same root cause). **Effort: medium.**

**Root cause.** `cast_kind` early-returns `None` for anything that isn't a bare `TypeExpr::Name`, and its name list covers only 10 scalars. A **complete** `TypeExpr → Kind` converter already exists and is used by `DEFINE FIELD … TYPE`: `schema.rs:945 kind_from_type_expr` (+ `base_kind_for_name:1070`, `parameterized_kind:987`).

**Repro.** `<record<person>> 'person:x'`, `<array<int>> [1]`, `<set<int>> […]`, `<option<string>> 'a'`, `<object> {}`, `<geometry<point>> …`, `<int | string> 1`, bare `<array>`/`<record>`/`<geometry>` — **all `unknown`**; `<int>`/`<string>` work.

**Expected:** the cast target verbatim. Codegen already renders all of these correctly (verified by routing the same kinds through the schema path).

**Also missed:** `CREATE user SET name = <record<user>> 'user:1'` emits **no E2001** — the `any` disables assignment checking. And `<future> { … }` is a genuinely separate sub-item: `grammar.js:2599` defines `_kw_future` but nothing references it, so it lowers as a cast to a name `future`.

**Corpus:** ~100+ parameterized casts (33× `<record<account>>`, 8× `<record<currency>>`, …), e.g. `@api/organization/setup_resume.surql:6`.

**Implementation caution.** Delegating to `kind_from_type_expr` requires threading `ctx.source_text()` into `cast_fact` and widening visibility. Once targets resolve, `check_cast`'s validity matrix (`check.rs:139, :236-246`) goes live for many more targets — audit for new E2008 FPs before shipping.

**Touch:** `crates/workspace/src/analyzer/expression/infer.rs:849-879`, `crates/workspace/src/schema.rs:945`, `crates/workspace/src/analyzer/expression/check.rs:139`.

---

### TI-7 — Whole operator families fall to `BinaryOp::Other` and infer `any`
**Merged from:** 2 filings (containment/pattern/IS; `**`). **Effort: small.**

**Root cause.** `lower/expr.rs:603` models only `+ - * / = == != < <= > >= AND && OR || ??`; everything else becomes `BinaryOp::Other(text)`. `binary_result_kind` has no `Other` arm ⇒ `_ => None` ⇒ `Kind::Any`.

**Affected:** `IN`, `NOT IN`, `CONTAINS*`, `INSIDE`, `OUTSIDE`, `ALLINSIDE`, `~`, `!~`, `?=`, `*=`, `IS`, `IS NOT`, `@@`, `<|k|>` — all should be **`Kind::Bool`**. Plus `**` (Pow), and the Unicode aliases `×` / `÷` the grammar accepts at `grammar.js:1781` (both currently `unknown`).

**Repro / missed diagnostic.**
```surql
CREATE user SET name = 1 > 2;      -- error[E2001] (correct)
CREATE user SET name = 1 IN [1,2]; -- NOTHING
CREATE user SET name = 2 ** 3;     -- NOTHING
```
**Expected:** the comparison arm's own comment states the principle — "a comparison produces a bool no matter what it compares; mismatched operands violate an invariant, not the result type." Same for containment/pattern/identity. `**` reuses `numeric_result` (SurrealDB's `Number::pow` preserves numeric kinds exactly like `*`), and non-numeric operands should raise E2004.

**Two hard constraints.**
- **Do NOT** add a catch-all `Op::Other(_) => Bool` — `Other` is a general escape hatch; use an explicit allowlist so unknown future text still falls through.
- **Do NOT** include `AND`/`OR` (one filing asked for this). SurrealQL's `AND`/`OR` are truthiness-based *value-returning* operators — `1 AND 2` evaluates to `2`, not `true`. The existing `both are Bool` guard is correct; `check.rs:410` already documents this.

**Touch:** `crates/syntax/src/lower/expr.rs:603-620`, `crates/workspace/src/analyzer/expression/infer.rs:886-957`, `crates/workspace/src/analyzer/expression/check.rs:359, 408`.

---

### TI-8 — Range expressions are entirely unmodeled
**Merged from:** 2 filings (for-controlflow + expr-builtins). **Effort: small–medium.**

**Root cause.** There is no `Range` node in the AST at all. The grammar *has* it (`grammar.js:1785 Range`, `:2238 RangeOp`) and puts it in the FOR-iterable `choice`, but `lower/expr.rs` has no `"Range"` arm, so it becomes `Expr::Partial` — **and its operand subtree is discarded entirely**.

**Symptoms.** `RETURN 1..10` → `unknown`. `FOR $i IN 1..10 { RETURN $i; }; RETURN 'z';` → the `any` loop var absorbs the block's whole exit union, so the trailing `string` vanishes. `SELECT age..nosuchfield AS e FROM user` → **no E1002** (subtree never walked). `CREATE t SET int_field = 1..10` → **no E2001**. Bonus: `[1,2,3][0..2]` is confidently **wrong** (`number` instead of `array<number>`) because the slice hits the `IdiomPart::Partial` catch-all.

**Expected.** `Kind::Range` (unit variant, already produced by `type::range()`, rendered at `query.rs:501`, mapped in codegen). Int-bounded ranges give the FOR var `Kind::Int`. `"range" => Kind::Range` also needs adding to `base_kind_for_name` — `DEFINE FIELD r ON t TYPE range` currently degrades to `any` while `check.rs:167 TYPE_NAMES` accepts the name.

**Cheapest complete path** (avoids touching every exhaustive `Expr` match): lower bounded `Range` to `Expr::Binary` with `BinaryOp::Other("..")`, add one arm in `binary_result_kind`. That fixes both the kind and the operand traversal.

**Caveat on the claimed payoff:** codegen deliberately maps `Kind::Range => "unknown"` (`crates/codegen/src/lib.rs:49`), so `out.ts` is unchanged until codegen + the client `Range` type are separately updated. The win here is diagnostics, not TS.

**Touch:** `crates/syntax/src/lower/expr.rs`, `crates/workspace/src/analyzer/expression/infer.rs:886`, `crates/workspace/src/analyzer/flow/for_loop.rs:29`, `crates/workspace/src/schema.rs:1070`.

---

### TI-9 — Closures: declared `-> T` disables body analysis; `$f(args)` invocation is untyped
**Merged from:** 2 filings. **Effort: small (a) / medium (b).**

**(a) Annotated closure bodies are never walked.** `closure_return_kind` returns the declared kind *before* the `with_child_env` body walk, and body diagnostics are only emitted as a side effect of that walk. So `|$n: int| -> int { RETURN (SELECT nofield FROM user); }` reports **nothing**, while the unannotated form correctly reports E1002 — **adding a type annotation strictly reduces checking**. The annotated `DEFINE FUNCTION` equivalent emits *both* E1002 and E2012, so the parity target already exists.
*Fix:* reorder (walk body, then return declared) + copy the ~25-line E2012 check from `schema/define/function.rs:57-88`. `ctx.emit` dedupes, so the repeated walk is safe — the IF-expression arm at `infer.rs:98-110` already establishes this exact pattern. **Small.**

**(b) `$f(3)` yields `any`.** The grammar parses `seq($.VariableName, $.ArgumentList)`, but `lower/expr.rs:246-261` reads only `FunctionName`, discarding the callee → `Call { path: "" }`; `function/mod.rs:47-53` then short-circuits to `Kind::Any`. `Kind::Function(params, ret)` **is** sitting in the env unread (proven: `array::map([1,2], |$n| -> string {…})` correctly gives `Array<string>`). Arity/arg-kind checks (5001/5002) are also entirely absent. **Medium** — needs an `ast::Call` field + lowering change before inference.

*Correction to the record:* `docs/plans/2026-07-24-kind-any-audit.md:183` files the empty call path as LEGITIMATE ("no name to resolve"). That's a fact about today's lowered AST, not intrinsic unknowability, and it contradicts G1 in the diagnostics backlog. Reclassify.

*Genuinely unknowable and correctly so:* the corpus's `LET $priority = function($source) {…}` (`organization/@functions.surql:44`) is a **JavaScript** `function(){}` block, not a SurrealQL closure. It stays `unknown` after this fix. Don't cite it as impact.

**Touch:** `crates/workspace/src/analyzer/expression/infer.rs:121-176, 836`, `crates/syntax/src/lower/expr.rs:246`, `crates/syntax/src/ast/expr.rs`.

---

### TI-10 — Untyped recursive UDFs collapse to `any` instead of taking the fixpoint
**Effort: medium. Severity: low — do this last.**

**Root cause.** PRE-PASS 1b infers untyped fn returns in source order; the in-flight self-call resolves to `Kind::Any` (`function/mod.rs:88-101`), `Flow::into_kind` lets `Any` absorb the exit union, and `infer_untyped_return` maps `Some(Any) → None`.

**Repro.** `fn::rec` with `IF $n <= 1 { RETURN 'base'; }; RETURN fn::rec($n-1);` → `unknown`. The byte-identical body with `-> string` → `string`.

**Honest impact assessment:** the one corpus candidate (`fn::organization::unit::path`) is **misattributed** — stubbing out both the self-call and `record::id` still leaves it `unknown`; the residual cause is an untyped `SELECT VALUE path FROM ONLY $x.parent`. This fix resolves **zero** corpus cases today.

**Why medium not small:** needs a bottom kind that `Flow::into_kind` drops without collapsing (distinct from `Kind::None`), SCC iteration for mutual recursion, and a convergence cap falling back to `Any` for non-tail recursion (`RETURN fn::rec($n-1) + 1`). `FunctionDef.callees` + the three-color DFS already exist to build on.

**Touch:** `crates/workspace/src/schema.rs:825-847`, `crates/workspace/src/analyzer/schema/define/function.rs:22-47`, `crates/workspace/src/analyzer/flow/block.rs:54-61`.

---

## B. Typegen / codegen row shapes

### TG-1 — Wildcard and mutation rows omit the implicit `id` (and `in`/`out` on RELATION tables)
**Merged from:** 5 filings (typegen-inventory, select-projection, mutation-return, graph-references, schema-extraction). **Effort: small code / medium test fallout.**

> **FIXED (2026-07-25).** `object_kind_for_field_prefix` injects `id` (and `in`/`out`
> when `table.relation.is_some()`) at `prefix == []` only, via `entry().or_insert()`
> so a declared `DEFINE FIELD id/in/out` wins. That covers all six materialized-row
> callers at once: `SELECT *`, every mutation return row, FETCH/graph
> materialization, and `DEFINE EVENT`'s `$before`/`$after`/`$value`.
> **Deliberately excluded:** nested objects inside a row (a sub-object is not a
> record); `SELECT VALUE`; explicit projections that didn't ask for `id`;
> `RETURN NONE`/`RETURN DIFF`; and **wildcard-under-GROUP** — a grouped row is
> synthesized from group keys and accumulators, not materialized, so
> `object_kind_for_declared_fields` is used there instead. (That a wildcard under
> GROUP still lists every declared field remains a separate pre-existing gap; the
> carve-out only declines to stack a new false claim on top of it.)
> `SELECT * OMIT id` stops being a silent no-op, and W7007 on `SELECT *, id`
> becomes truthful. Oracle unchanged at 40 findings with the same distribution.

**Root cause.** `object_kind_for_field_prefix` iterates `table.fields.values()` only. `TableDef::implicit_field_kind` — which already returns `record<self>` for `id` and the FROM/TO record kinds for `in`/`out` — is reached only from `kind_for_path`/`resolve_field_path`, i.e. the *explicit-projection* path. That's exactly why `SELECT id` works and `SELECT *` doesn't. Its doc comment ("they live outside `self.fields` so the schemaless `fields.is_empty()` gate stays untouched") explains *where* they're stored, not a decision to exclude them from `*`.

**Symptom list (all one fix):**
- `SELECT * FROM user` → `Array<{ name: string }>`, no `id`
- `SELECT * FROM likes` (RELATION) → no `id`/`in`/`out`
- `CREATE/UPDATE/UPSERT/INSERT … RETURN AFTER`, `RETURN *`, `RETURN BEFORE`, `RELATE …` → all missing `id`
- `LET $c = CREATE ONLY …; RETURN $c.id` → `unknown`
- `SELECT * OMIT id` is a silent no-op (and emits no diagnostic)
- `DEFINE EVENT` `$before`/`$after` are missing `id`
- FETCH materialization (`materialize_record_kind`) likewise

**Internal contradiction proving intent:** `SELECT *, id FROM a` emits **W7007 "this field is already included by `*`"** while inferring a row type with no `id`. The user is told to delete the only projection that would have typed their primary key.

**Corpus:** zero `DEFINE FIELD id ON …` across 124 tables; 20 `TYPE RELATION` tables. So 100% of corpus row types are affected.

**Soundness audit (done).** All 6–7 callers of `object_kind_for_all_fields` are **output/materialized-row** contexts, never input-content validation — `select.rs:70, :1335, :1646`, `mutation.rs:666, :696`, `schema/define/event.rs:24`. So injecting `id`/`in`/`out` **cannot** produce a false "missing required field" on `CREATE … CONTENT`. `apply_omit` runs after the row kind is built, so `OMIT id` keeps working (and becomes meaningful). `implicit_field_kind` degrades gracefully to `record<>` for unconstrained endpoints.

**Fix.** Inject at `prefix == []` only (nested prefixes must not get an `id`), guarded so a declared `DEFINE FIELD id`/`in`/`out` wins; `in`/`out` only when `table.relation.is_some()`. Leave the schemaless `fields.is_empty()` gates alone.

**Touch:** `crates/workspace/src/analyzer/data/select.rs:1715-1750`, `crates/workspace/src/schema.rs:461`.

---

### TG-2 — A wildcard projection swallows every sibling projection
**Effort: small.**

**Root cause.** `select_response_kind` short-circuits to `object_kind_for_all_fields` whenever *any* projection is a `Wildcard`, never entering `projected_object_kind` — whose own `Wildcard(_) => {}` no-op arm is correct-for-a-merge but unreachable. Duplicated on the mutation RETURN path.

**Repro.** `SELECT *, age + 1 AS next FROM user` → `Array<{ age; name }>`; `next` is silently gone. Same for `*, name AS alias`, `*, ->wrote->post AS posts`, `*, count() AS c … GROUP ALL`, and `UPDATE … RETURN *, age + 1 AS next`. Control `SELECT age + 1 AS next` alone → `{ next: number }`. **Zero diagnostics.**

**Expected.** SurrealDB merges; overlay the projected map onto the wildcard map (later explicit projection wins on collision). This is a *missing key*, so downstream `tsc` rejects `result[0][0].next` — a false type error, worse than an over-wide type.

**Touch:** `crates/workspace/src/analyzer/data/select.rs:65-75, :1030`, `crates/workspace/src/analyzer/data/mutation.rs:692-697`.

---

### TG-3 — A fieldless SCHEMAFULL table degrades the whole statement to `unknown`
**Effort: medium.**

**Root cause.** `select.rs:205` gates on `table.fields.is_empty()` alone, never on `table.schemafull`, and short-circuits to a findings-only walk with no kind. The same assumption is duplicated at `data/mod.rs:64` (kills E1002 on closed tables), `mutation.rs:663`, `select.rs:1646`.

**Repro.** `DEFINE TABLE empty_rel SCHEMAFULL TYPE RELATION FROM a TO c;` → `SELECT id FROM empty_tbl`, `SELECT in, out FROM empty_rel`, `SELECT count() … GROUP ALL`, `SELECT 1 AS one FROM empty_tbl`, `CREATE empty_tbl` — **all `unknown`**. Plus: `SELECT * FROM empty_tbl WHERE nonexistent = 1` emits **no E1002** even though SCHEMAFULL is closed.

**Corpus:** `works_at` (`TYPE RELATION IN employee_of OUT outlet`, no payload fields) and `with_discount` — and the corpus *queries* one: `apps/commerce/outlet/@function.surql:10 LET $link = SELECT VALUE id FROM works_at …`, an untypable LET in an all-valid corpus. 7008's help ("add DEFINE FIELD declarations for full analysis") is actively misleading advice for payload-free edges.

**Expected.** Explicit projections, literal aliases, `count()`, CREATE row shape all resolve; closed-table field diagnostics fire. The `SELECT *` half is answered by TG-1 (it becomes `{ id: … }` / `{ id, in, out }`).

**Touch:** `crates/workspace/src/analyzer/data/select.rs:205, :1646`, `crates/workspace/src/analyzer/data/mod.rs:64`, `crates/workspace/src/analyzer/data/mutation.rs:663`.

---

### TG-4 — FETCH silently no-ops on `option`/`set` links and on nested paths
**Effort: medium.**

**Root cause.** `materialize_record_kind` matches only `Kind::Record` and `Kind::Array` — no `Option`, no `Set` arm — while `kind_may_hold_record` (used by the W1023 "FETCH does nothing" check) *does* accept them. The two disagree, so the analyzer classifies the link as record-holding, declines to warn, and then fails to materialize. `materialize_at_path` recurses only into object literals, so it stops dead on a `Kind::Record` prefix.

**Repro.** `FETCH lead` (`option<record<user>>`), `FETCH tags` (`set<record<user>>`), `FETCH maybe_many` (`option<array<record<…>>>`), `FETCH author.team` — all unchanged, **zero diagnostics**. Controls `FETCH author` (bare record) and `FETCH members` (array) materialize correctly.

**Ground truth checked against surrealdb-core 3.2.1** (`exec/operators/fetch.rs`): `fetch_field_path` explicitly handles "Mid-path RecordId: fetch in place and retry at the same depth", and `fetch_value_if_record` handles `Value::RecordId` and `Value::Array`. So the expected shapes are the real runtime shapes. Further proof it's a missing recursion, not a boundary: `FETCH author, author.team` (both listed) **does** work.

**Impact:** the emitted TS is not loose but **wrong** — it claims `RecordId<"user">` where the runtime value is a full object. 106 `option<record<` fields in the corpus.

**Touch:** `crates/workspace/src/analyzer/data/select.rs:1615, :1639, :258`.

---

### TG-5 — Array subscripts (`[0]`, `[*]`) in a projection discard the element type
**Effort: medium.**

**Root cause.** `project_field` runs the checker for side effects and then hard-codes `Kind::Any` for any idiom containing a non-`Field` part — the comment literally says *"Idioms with parts we don't project yet (Start/Index/Method/...)"*. `plain_field_segments` returns `None` on an `Index`, so `resolve_field_path` is never reached.

**Decisive control:** wrapping the identical expression in parentheses routes it through `computed_kind` and produces **exactly the expected types**:
```
SELECT tags[0] AS x        -> { x: unknown }      SELECT (tags[0]) AS x2       -> { x2: string }
SELECT links[0].name AS n  -> { n: unknown }      SELECT (links[0].name) AS q  -> { q: string }
SELECT links[*].name AS m  -> { m: unknown }      SELECT (links[*].name) AS q2 -> { q2: Array<string> }
```
The inference machinery already resolves Index/Star over `Kind::Array` in row-table context **including link crossing and star-mapping**. The projection path throws it away.

**Corpus:** `sectors[0].name`, `channels[0]` on `discount` etc. → `unknown`.

**Two traps that make this medium, not a one-line fall-through:**
1. Range subscripts are currently inferred **too narrow** in the expression path (`(tags[0..2])` → `string`, must be `Array<string>`). A blanket fall-through propagates that unsoundness into projections.
2. Unaliased projections have a separate key-naming bug: `SELECT tags[0] FROM team` → key `"tags[0]"`. SurrealDB names these by the idiom's name parts (`tags`, nested `links.name`), so `insert_kind_at_path` needs index-aware segment extraction.

**Touch:** `crates/workspace/src/analyzer/data/select.rs:1146, :1755, :1793`.

---

### TG-6 — Projections that descend into an inline literal-object field yield `unknown`, keyed by the raw dotted path
**Effort: medium.**

**Root cause.** `kind_for_path` resolves declared sub-field prefixes, exact `a.b` keys, and record-link crossing — a head whose declared kind is `Kind::Literal(KindLiteral::Object(..))` falls to the `_ => None` arm. `project_expr` then hits the poison branch `fields.insert(segments.join("."), Kind::Any)`. The capability exists 300 lines away: `infer.rs:505 Kind::Literal(Object(fields)) => fields.get(field)`.

**Repro.**
```surql
DEFINE FIELD lit ON t TYPE { a: string, b: int, n: { deep: bool } };
SELECT lit.a FROM t;            -- { "lit.a": unknown }   (wrong key AND unknown)
SELECT lit.a AS x FROM t;       -- { x: unknown }
SELECT VALUE lit.a FROM t;      -- degrades to an OBJECT row, worse than claimed
SELECT lit.nope FROM t;         -- no E1002 on a provably-closed object
SELECT obj.a FROM t;            -- WORKS (DEFINE FIELD obj.a sub-field style)
RETURN (SELECT VALUE lit FROM ONLY t:1).n.deep;  -- WORKS (expression path descends, multi-level)
```
**Corpus:** `account.settings` is an inline literal object; `SELECT settings.theme FROM account` → `{ "settings.theme": unknown }`, while `SELECT settings` gives the full literal union.

**Expected.** Nest under the path head (the analyzer's own convention for `obj.a`): `{ lit: { a: string } }`; aliased → `x: string`. Must also unwrap `option`/`array` around the literal, keep `validate_field_path`/`field_is_opaque_boundary` in sync (E1002 starts firing on bogus members — needs a corpus FP run), and fix the VALUE path.

**Scope correction — do NOT bundle `.*`.** `SELECT obj.* FROM t` is equally unhandled with the *sub-field* style, and SurrealDB's `.*` on an object yields the object's **values**, not its members — so "expand the literal object's members" is likely the wrong expected output. Separate, lower-confidence item.

**Touch:** `crates/workspace/src/analyzer/data/select.rs:1146, 1722-1760, 1900`, `crates/workspace/src/schema.rs:449`.

---

### TG-7 — Mutation default/BEFORE response kinds contradict the engine (and the analyzer's own warning)
**Merged from:** 2 filings (DELETE default; CREATE RETURN BEFORE). **Effort: small.**

**(a) DELETE's default return.** Typed as the deleted rows; SurrealDB returns **nothing** per row. Verified in surrealdb-core 3.2.1 `doc/output.rs:170-186`: with `output == None` the arm lists only `Create | Upsert | Update | Relate | Insert => output_after(...)`; everything else hits `_ => Err(IgnoreError::Ignore)`. Same in 1.5.0 `doc/pluck.rs:49-70`, so this is **not** a version-gated decision. `DELETE ONLY` with empty result → `Value::None` (`expr/statements/delete.rs:107-113`). Expected: `Kind::Array(Any, Some(0))` / `Kind::None` under ONLY — exactly what `RETURN NONE` already produces. **89 DELETE occurrences in the corpus.** The wrong type actively invites `.map` / `[0].name` over an always-empty result. The analyzer already self-flags this at `delete.rs:3-5` and `mutation.rs:659-661` as "unverified" — it is now verified.

**(b) CREATE … RETURN BEFORE.** Emits `warning[W4020]: … is always NONE` and simultaneously types the result as a full row. `output_before` returns `initial.doc`, and `is_new()` is literally `initial.doc.is_none()` for a create. BEFORE is *not* swallowed (only `Output::None` is), so the shape is `[NONE]` per row → `Kind::Array(Kind::None)` / `Kind::None` under ONLY. **Caveat to encode:** `output_before` still runs `compute_fields(DocKind::Initial)`, so on a table with COMPUTED fields the NONE initial doc can materialize into an object of just the computed fields — safe implementation is `Kind::None` when the table has no COMPUTED fields.

**Adjacent, same class, no diagnostic exists:** `RELATE … RETURN BEFORE` also always creates a fresh edge and currently types as a full row. Also flagged for review: `DELETE … RETURN AFTER` types as full rows, but upstream `output_after` runs after `clear_record_data()`.

**Touch:** `crates/workspace/src/analyzer/data/mutation.rs:640-673`, `crates/workspace/src/analyzer/data/delete.rs:17`, `crates/workspace/src/analyzer/data/create.rs`.

---

### TG-8 — `FROM (SELECT …)` with any non-wildcard projection collapses to a bare `unknown`
**Effort: medium. Marginal priority.**

**Root cause.** The `Expr::Subquery` arm of `resolve_from_table` does `if !all_wildcards { return Err(Kind::Any); }` *before* inferring the inner statement, and `select_response_kind` propagates that as the whole response — so even the `Kind::Array` wrapper is lost. The in-repo comment concedes it.

**Repro.** `SELECT name, age FROM (SELECT * FROM user)` → `[unknown]`. Also `AS`, `VALUE`, `ONLY`, and `SELECT bogus FROM (…)` (no unknown-field diagnostic). `SELECT * FROM (SELECT * FROM user)` works.

**Expected.** For `SELECT *` the inner element is already a concrete `Kind::Literal(Object)`; projecting off it is plain object-literal field access (`infer.rs:505`). Safe floor: `Array<any>` when the element isn't an object literal.

**Why medium:** `projected_object_kind`/`project_expr` are table-bound by signature, so this needs a parallel object-literal projection path covering Expr/alias/VALUE/ONLY.

**Honest priority note:** the corpus has **zero** `FROM (` subquery sources. The two-line "at least keep the `Array<…>` wrapper" mitigation is separately trivial and worth doing now.

**Touch:** `crates/workspace/src/analyzer/data/select.rs:130-158`.

---

## C. Diagnostic coverage

### DX-1 — `GROUP BY <alias>` / `ORDER BY <alias>` false-positive E1002
**Effort: small. Do this first in this group.**

**Root cause.** `select_response_kind:56-62` runs an unconditional `check_field_path(…, 1002)` over every GROUP BY key; `check_order_clause:417` does the same for ORDER BY. Both clauses' *other* checks (`check_group_key_projection:675` for W4013, `explicit_keys` for 2017) already build an alias-aware projected-name set — the earlier 1002 check just doesn't consult it.

**Repro (verified against live `surreal` 3.0.5, both queries execute fine):**
```surql
SELECT name AS n FROM user GROUP BY n;              -- error[E1002] `user` has no field `n`
SELECT team AS t, count() FROM user GROUP BY t;     -- error[E1002]
SELECT name AS n FROM user ORDER BY n;              -- error[E1002]   (ORDER BY half not in the original filing)
SELECT name AS n, count() FROM user GROUP BY n ORDER BY n;  -- TWO E1002s
```
**Fix.** Hoist the projected-name/alias set; skip the 1002 check when the key equals or is prefixed by a projection alias. W4013/2017 remain the correct owners of "key isn't a result field".

**Side observation (separate item):** SurrealDB 3.0.5 **hard-rejects** a non-projected group key ("Missing group idiom `name` in statement selection"). W4013's doc comment claims "SurrealDB runs the query (it does not reject this), which is why it is a warning" — that rationale is stale for 3.x; W4013 arguably should be an error.

**Touch:** `crates/workspace/src/analyzer/data/select.rs:55-62, 376-417, 675`.

---

### DX-2 — Aggregate column promotion fires only for a bare top-level `math::x(field)`
**Effort: medium.**

**Root cause.** `column_aggregate_kind` bails unless the projection *is* the `Expr::Call`, has exactly one arg, and that arg is a plain field idiom resolvable via `plain_field_segments`. Everything else falls through to per-row inference (`int`/`number`), which then violates the `array` contract in `analyze_builtin_function` → E5002.

**Repro (all error, all valid).**
```surql
SELECT math::sum(price * qty) AS revenue FROM post GROUP ALL;
SELECT math::sum(age) * 2 AS d FROM user GROUP ALL;
SELECT math::sum(price) + math::sum(qty) AS a FROM post GROUP ALL;   -- TWO E5002s
SELECT math::mean(age * 1) AS a FROM user GROUP ALL;
SELECT math::sum(<float>age) AS a FROM user GROUP ALL;
```
**Ground truth (surrealdb-core 3.2.1, checked not guessed):** `exec/planner/aggregate.rs:306 AggregateExtractor` is a **recursive** `MutVisitor` that finds an aggregate call at *any* depth and substitutes a synthetic field idiom, keeping the surrounding expression. `exec/operators/aggregate.rs:171` holds `argument_expr: Arc<dyn PhysicalExpr>` — "the expression to evaluate per-row to get the value to accumulate". Both shapes are fully supported SurrealQL.

**Fix.** Part one (accept any argument expression, infer under `with_row_table`, wrap in `Kind::Array`, keep the "already a collection" guard) is a localized rewrite. Part two (recognize the aggregate at any depth) needs a grouped-projection flag in the inference context or a mirror of core's extract-and-substitute pre-pass — structural, hence medium.

**Two decisions to make in the same change:** the existing promotion is **not** gated on `stmt.group` at all (`SELECT math::sum(age) FROM user` is silently clean) — recommend keeping it ungated to avoid a new regression class. And core rejects *nested* aggregates, which is a separate missing-diagnostic opportunity.

**Touch:** `crates/workspace/src/analyzer/data/select.rs:1177, 1208, 1243`.

---

### DX-3 — Required-field check (2034) keys off object literals only — FP in one direction, FN in the other
**Merged from:** 2 filings. **Effort: medium.**

**Root cause.** `provided_field_names` → `object_keys`, which returns an **empty `Vec`** for anything that isn't an `Expr::Object`. The `Vec<String>` return type conflates *"provides nothing"* with *"key set unknown"*.

**False positives (error severity, aborts `generate`):**
- `CREATE person CONTENT $payload;` → E2034 for **every** required field.
- `LET $p = { name:'a', age:1 }; CREATE person CONTENT $p;` → same two errors, **even though the analyzer knows the exact shape** (`RETURN $p` → `{ age: number; name: "a" }`). Pure information-available FP with no "unknowable" defence.

**False negatives:**
- `INSERT INTO person [{ name:'a' }]` — `object_keys` returns `[]` on the `Expr::Array` and the `!keys.is_empty()` guard swallows it, even though `check_insert_payload` already destructures that same array per-row.
- `UPDATE/UPSERT … CONTENT|REPLACE` never call `check_required_fields` at all. CONTENT/REPLACE replace the record wholesale, so an omitted non-optional, non-defaulted field becomes NONE and SurrealDB hard-fails.

**The correct policy already exists twice in the same module** (`check_payload_object_keys` bails on non-object; `insert.rs:174-177` guards on `!keys.is_empty()`) — CREATE is the only path missing the guard, which rules out "by design".

**Fix.** `provided_field_names → Option<Vec<String>>` (`None` = unknown key set, `Some(vec![])` = no data clause); derive keys from a `Kind::Literal(Object)` when the payload is LET-bound; constrain an unbound `$payload` to the table's required-row object kind via the existing `ctx.constrain_param` (already used for SET values); destructure `Expr::Array` in insert.rs; add a **Content|Replace-only** helper for UPDATE/UPSERT (MERGE must keep its exemption — it's a partial patch).

**Touch:** `crates/workspace/src/analyzer/data/mutation.rs:102, 147, 165`, `create.rs:38-42`, `insert.rs:163`, `update.rs:16`, `upsert.rs:16`.

---

### DX-4 — Write-contract checks are wired to some data clauses and not others
**Merged from:** 2 filings (READONLY/computed on SET only; PATCH unchecked). **Effort: medium.**

**Root cause.** `check_field_write_flags` is invoked *only* from the `DataClause::Set` arm. The `Content|Merge|Replace` arm calls only `check_payload_object_keys`; `InsertData::Assignments` calls only `check_field_path`; and the `Patch` arm calls `check_patch_operations`, which **takes no `&TableDef` at all** — even though `row_table` is in scope and is passed to every sibling arm.

**Coverage matrix as it stands:**

| Clause | field exists (1002) | value type (2001) | READONLY (2025) | computed (2026) |
|---|---|---|---|---|
| SET | ✅ | ✅ | ✅ | ✅ |
| CONTENT / MERGE / REPLACE | ✅ | ✅ | ❌ | ❌ |
| INSERT `ON DUPLICATE KEY UPDATE` | ✅ | ❌ | ❌ | ❌ |
| PATCH | ❌ | ❌ | ❌ | ❌ (only opcode via E2033) |

**Extra bug found in the same area:** `lower/statement.rs:616` sets `stmt.data = InsertData::Assignments(assignments)` whenever *any* FieldAssignment is present, so an `ON DUPLICATE KEY UPDATE` clause **discards the VALUES/columns payload entirely** — silently disabling E2001 *and* 2034 for that whole INSERT. Needs an AST change to carry ON DUPLICATE assignments separately.

**PATCH specifics.** A JSON Pointer `/a/b` maps cleanly to segments `["a","b"]`, which is exactly what `kind_for_path` consumes, and `check_field_path` + `kind_is_assignable_to` are the existing templates. Traps that must be handled or it FPs: `check_patch_operations` currently walks object keys independently and must correlate op/path/value per operation; array index and append tokens (`/tags/0`, `/tags/-`) must bail or resolve through the element kind; `add`/`replace`/`test` value kind is the *element* kind at an array position; `move`/`copy` carry `from`, `remove` carries neither.

**Soundness.** Writing a READONLY field on a non-creating statement is a hard runtime error and a VALUE-computed field is always overwritten — the clause form is irrelevant to the contract. `creating` is already threaded through, so CREATE/INSERT keep suppressing 2025 while still warning 2026. Corpus FP check: the two READONLY fields and the handful of MERGE payloads don't collide, so zero new corpus findings.

**Touch:** `crates/workspace/src/analyzer/data/mutation.rs:51-96, 307-338, 344, 568`, `insert.rs:138`, `crates/syntax/src/lower/statement.rs:616`.

---

### DX-5 — A mutation target that isn't a literal table/record-id skips **all** payload checking
**Effort: medium. High severity — silent false negatives on writes.**

**Root cause.** `source_table_name` matches only `Expr::Table` and `Expr::RecordId`, takes no `&AnalysisContext`, and therefore cannot consult an inferred kind. Every mutation analyzer does `let Some(t) = … else { return Kind::Any }`, which simultaneously poisons the response kind **and** passes `table_hint = None` into `analyze_expression_positions_for` — whose payload checks are all gated on `if let Some(table) = row_table`.

**Repro.**
```surql
LET $r = person:one; UPDATE $r SET age = 'oops';            -- ZERO diagnostics
UPDATE type::thing('person','x') SET bogus = 1;             -- ZERO diagnostics
DEFINE FUNCTION fn::touch($p: record<person>) { UPDATE ONLY $p SET age = 2; };
```
Controls: `UPDATE person:one SET bogus = 1` → E1002; `SET age = 'oops'` → E2001. And `LET $r2 = person:one; RETURN $r2` → `RecordId<"person">` — **the table name is in hand**.

**SELECT already does this.** `select.rs:150-162` resolves an `Expr::Param` source via `let_fact(param).kind == Kind::Record([t])` with the comment "that IS the source table — project against it like a plain table". `SELECT * FROM $r` works today. So SELECT and the mutations disagree — an internal inconsistency, not a design stance.

**Sub-gap:** `type::record`/`type::thing` return `Kind::Record(vec![])` unconditionally (`function/type_/record.rs:21`). Narrowing the 2-arg form with a literal-string first arg to `Kind::Record([t])` fixes read and write paths at once and is sound (that arg *is* the table).

**Implementation caveat (checked).** Make this a **new ctx-aware resolver used only for the table hint + response kind**. `check_only_on_table` and `check_whole_table_write` match `Expr::Table` syntactically and are unaffected — so no spurious 7009 on `UPDATE ONLY $p`.

**Scope note:** `@api/account.surql:16 UPDATE ONLY $has_email SET …` is *weaker* than filed — `$has_email` is a `SELECT … FROM ONLY` row object, not a `record<T>`. Scope to (a) `Kind::Record([t])` targets and (b) literal-table `type::thing`/`type::record`; leave the row-object case out.

**Touch:** `crates/workspace/src/analyzer/data/mutation.rs:756`, `update.rs:27-35`, `create.rs:28-36`, `delete.rs`, `upsert.rs`, `insert.rs`, `relate.rs`, `function/type_/{record,thing}.rs`.

---

### DX-6 — Table-level PERMISSIONS bodies are analyzed against an empty row scope
**Effort: medium.**

**Root cause.** `analyze_permission_predicates` is **shared** by table.rs and field.rs and already does everything right (`ctx.with_row_table`, binds `$value/$this/$before/$after/$auth/$session`, runs infer + check + `check_expression_field_paths(1002)` + the 2005 non-bool check). It fails only at `permissions.rs:48 let row_table = ctx.schema().tables.get(table_name)` — at that moment the table's fields (declared by *later* `DEFINE FIELD`s) don't exist yet, so it looks schemaless and everything is skipped. The module's own doc comment (lines 19-27) documents the punt.

**Decisive control.** Re-order so the fields are already in the catalog and **both** checks fire on the table-level clause: `E5002 … but $n is declared int` and `E1002 doc has no field ghost`. The machinery is fully built; only the catalog is empty. Confirmed it is **not** intra-file ordering — silent across sources in both orders.

**Corpus:** 114 PERMISSIONS occurrences across 95 files, dominated by exactly this shape — `FOR SELECT WHERE fn::organization::permissible(organization, 'roles/read')`, `fn::entity::permissible(calendar,'read')`, `in.in = $auth`, `owner = $auth`. All currently unchecked.

**FP risk probed:** on a RELATION table, implicit `in`/`out`/`id` resolve with correct record kinds (no false E1002); session params stay open shapes.

**Fix.** Re-walk permission predicates after the global catalog exists — direct precedent in PRE-PASS 1b (global fn returns) and 1c (global untyped-field kinds), both added for this same "built against an empty catalog" class. Medium because it needs correct source/span/env wiring on the re-walk.

**Touch:** `crates/workspace/src/analyzer/schema/define/permissions.rs:48`, `crates/workspace/src/analyzer/pipeline.rs`.

---

### DX-7 — ASSERT is never enforced against a written const value (E2037 does exactly this for DEFAULT)
**Effort: medium.**

**Root cause.** The full const-ASSERT evaluator exists but is private to one module and wired only to DEFAULT: `enum ConstVal`, `fold_const`, `eval_assert`, `eval_binary` (AND/OR short-circuit, Eq/NotEq/Lt/Gt/…, `IN`/`INSIDE`/`CONTAINS` via `eval_membership`), `operand_val` (binds `$value`) — all in `define/field.rs:334-500`, consumed only by `check_default_satisfies_assert`.

**Repro.** `ASSERT $value >= 0` + `CREATE acct SET balance = -10` → **silent**. `ASSERT $value IN ['open','closed']` + `status = 'bogus'` → **silent**. Same for CONTENT and INSERT. Meanwhile `DEFINE FIELD bad … DEFAULT -5 ASSERT $value >= 0` correctly emits E2037. Same predicate, same constant, checked in one position and not the other.

**Corpus:** 173 ASSERT clauses, 118 of them `ASSERT $value IN [...]` closed sets — exactly what `eval_membership` already decides. A typo'd literal is undetectable today.

**Why medium.** `FieldDef` doesn't store the ASSERT at all, and it's `Serialize`/`Deserialize` (cached schema), so raw AST can't be stashed — needs a distilled serde-able predicate. Plus: must **skip `computed` fields** (the value is overwritten before the ASSERT runs; 2026 already covers that case) or it FPs; per-element `field.*` ASSERTs bind `$value` to each element; and wiring spans SET, CONTENT/MERGE object keys, and INSERT.

The evaluator is bail-first by construction (`eval_membership` returns `None` unless every member folds; emits only on `Some(false)`), so reuse is exactly as sound as the DEFAULT case.

**Touch:** `crates/workspace/src/analyzer/schema/define/field.rs:334-500`, `crates/workspace/src/schema.rs:176-203`, `crates/workspace/src/analyzer/data/mutation.rs:438, 568`, `insert.rs`.

---

### DX-8 — `check --json` drops every warning when `errors == 0`
**Effort: small. Not in the original 56 — found while verifying SX-7. Fix immediately.**

**Repro.** A project whose only finding is a warning: `{"summary":{"diagnostics":1,"errors":0},"diagnostics":[]}`. Add one error-severity finding and the array populates.

**Impact.** Every `--json` consumer and CI integration silently loses all warning-only runs — including every hint the analyzer emits about its own limitations (see SX-7's H6003).

**Touch:** the `--json` serialization path in `crates/cli/src/main.rs`.

---

### DX-9 — FOR over an object / record id / geometry is not flagged (E2022 is a scalar-only deny-list)
**Effort: small.**

**Root cause.** `for_loop.rs:36-61` is `matches!(base, Int | Float | Decimal | Number | Bool | String | Datetime | Duration | Uuid | None | Null)`. `Kind::Object`, `Kind::Record`, `Kind::Geometry`, `Kind::Bytes`, `Kind::File` have no arm.

**Ground truth (surrealdb-core 3.2.1 `expr/statements/foreach.rs:55-68`):** accepts exactly `Value::Array` and `Value::Range`; everything else is `Err(InvalidStatementTarget)`. So these are as definite as the already-flagged `string`. Catalog entry 2022 is already Error/Deny.

**Real-world shape worth catching:** `LET $u = (SELECT * FROM ONLY user LIMIT 1); FOR $x IN $u { … }` — the forgotten-ONLY mistake — is silently accepted today.

**Recommendation: extend the deny-list, do NOT invert to a positive contract** (one filing asked for inversion). Inversion would (a) false-positive on `FOR $i IN 1..10` since `Kind::Range` is essentially unmodeled (see TI-8), and (b) newly flag `Kind::Either` operands like `option<array<int>>`, a behavior change with FP risk. Deny-list extension is ~5 lines with zero FP risk; treat inversion as a separate decision after TI-8 lands.

**Touch:** `crates/workspace/src/analyzer/flow/for_loop.rs:36-61`.

---

### DX-10 — Redefinition warning 1022 covers only TABLE and FIELD
**Effort: medium.**

**Root cause.** 1022 has exactly two emit sites (`table.rs:18`, `field.rs:204`). function.rs / param.rs / event.rs / analyzer.rs / index.rs have no redefinition check, though the catalog entry is stated generically.

**Repro.** Duplicate `fn::dup`, `$pp`, event `de`, analyzer `da`, and a literal duplicate `DEFINE INDEX di` are all silent. The **type silently flips**: two `fn::dup` bodies returning `1` then `'two'` → `CREATE t SET n = fn::dup()` yields `E2001 int vs string` with no 1022 anywhere.

**Also inconsistent semantics:** `insert_table` without OVERWRITE **keeps the first**; `insert_param`/`insert_function`/`insert_analyzer` **replace** (last-wins). Two different silent resolutions of the same contract violation.

**Extra hole:** index 1029's duplicate-fields check explicitly excludes same-name, so two identical `DEFINE INDEX di` fall through both 1022 and 1029.

**Why medium.** Only `DefineTable`/`DefineField` carry `overwrite: bool` — `DefineIndex`/`Event`/`Param`/`Function`/`Analyzer` **parse and drop** OVERWRITE. It must be threaded through lowering first, or the check fires on legitimate code: the corpus uses `DEFINE INDEX OVERWRITE` **294×**, `FUNCTION OVERWRITE` 37×, `EVENT OVERWRITE` 21×, `PARAM OVERWRITE` 8×, `ANALYZER OVERWRITE` 3×. EVENT is costliest — `schema.rs:553` says "Events are validated but never stored", so it needs a new EventDef store or a per-source name scan.

**Touch:** `crates/syntax/src/ast/statement.rs:351-420`, lowering, `crates/workspace/src/analyzer/schema/define/{function,param,event,analyzer,index}.rs`, `crates/workspace/src/schema.rs:293-305, 410, 553`.

---

### DX-11 — 1012 covers only REBUILD/REMOVE INDEX: unknown ANALYZER and unknown `WITH INDEX` are silent
**Effort: medium (lowering gap, not a check-ordering gap).**

**Root cause.** Both names are **discarded at lowering**:
- `ast::DefineIndex` has no analyzer field; `lower_define_index` reads `IndexClause` only for a kind discriminant and drops the `Ident` that `grammar.js:1198/1210` produces.
- `WithClause` is in the grammar (`:986`) but explicitly listed in the ignore comment at `lower/statement.rs:281`; there is no `WithClause` reference anywhere in `crates/workspace`.

**Both names are fully resolvable** once lowered: `SchemaIndex::analyzer()` exists (SurrealDB ships **no** built-in analyzers, so every name must have a DEFINE in the workspace — same catalog-completeness assumption 1001/1002/1032 already make), and `TableDef::indexes` is already keyed. `AnalyzerDef` carries `name_span`, so did-you-mean is a drop-in.

**Bundled bug found while reproducing (fix in the same change).** `index_kind_from_clause` has **no `FulltextClause` arm**, so `FULLTEXT` indexes lower to `IndexKind::Normal` — producing a live **W1029 false positive** ("index `good` covers the same fields as `ft`") between a FULLTEXT index and a plain lookup on the same field, contradicting 1029's own documented rationale.

**Corpus:** `suite/task/task.surql:97 FULLTEXT ANALYZER english` against `@analyzers.surql` — a rename or typo in either place is undetectable.

**Touch:** `crates/syntax/src/ast/statement.rs:351`, `crates/syntax/src/lower/statement.rs:281, 1239`, `crates/workspace/src/analyzer/schema/define/index.rs:99-125`, `crates/workspace/src/analyzer/data/select.rs`.

---

### DX-12 — Catalog code 5010 (event trigger cycles) has no emission site
**Effort: large. Do after DX-10 lands the EventDef store.**

**Root cause.** Events are discarded entirely: `schema.rs:552 // Events are validated but never stored; the long tail is unmodeled.` The 5009 function-cycle analogue is fully built (three-color DFS, per-source injection, incremental invalidation in `analysis.rs` and `lsp/workspace.rs`) and is what should be mirrored.

**Two corrections to the proposed design — the naive table-only graph is unsound:**
1. **The filing's own mutual-cycle example is not a cycle.** `a2b ON user WHEN $event='CREATE'` / `b2a ON post WHEN $event='UPDATE' THEN (UPDATE user …)`: b2a writes user with an UPDATE, but a2b only fires on CREATE. Edges must be **operation-aware** — the WHEN clause's implied `$event` set ∩ the writing statement's operation kind.
2. **The oracle contains a same-table writer that must NOT be flagged:** `organization/organization_unit.surql:44 DEFINE EVENT set_path … WHEN $event='CREATE' OR $event='UPDATE' THEN { UPDATE organization_unit SET path = … }`. A naive check fires here. The 5009 guardedness carve-out (`fn_body_branches` — body contains IF/FOR ⇒ not provably divergent) does spare it, but it is **load-bearing, not optional**.

Also needed: WHEN→op-set extraction for implicit forms like `$before.used_at != $after.used_at` (implies UPDATE only).

**Touch:** `crates/workspace/src/schema.rs:552`, `crates/workspace/src/analyzer/schema/define/event.rs`, `crates/workspace/src/analyzer/pipeline.rs:494, 661, 966`, `crates/workspace/src/analysis.rs:1005`, `crates/lsp/src/workspace.rs:550`.

---

## D. Graph & reference traversal

### GR-1 — `<~T` reference back-links: E3001 false positive in queries, untyped with a tail/filter, and no diagnostic when provably dead
**Merged from:** 3 filings. **Effort: medium.**

Three linked defects around the `<~` reference operator. `GraphStep.reference` is set correctly at lowering (`lower/expr.rs:541`) and `grammar.js:2244 LookupLeft: choice('<-','<~')` treats it as a first-class, position-independent lookup — so a COMPUTED-only semantic model is the bug.

**(a) Query-side E3001 FP (error severity).** `graph.rs::check_step` branches on `tables.get(edge).and_then(|t| t.relation)` and **never inspects `step.reference`**, so `SELECT <~post AS back FROM person` → `error[E3001]: 'post' can't be traversed — it is not a relation table`, even when `post.author` is `record<person> REFERENCE`. The identical `<~post` in a `COMPUTED` clause types correctly as `Array<RecordId<"post">>`. **This is why the corpus has 22 `<~` sites and all 22 are in COMPUTED clauses, zero in queries** — the query form is unusable today. ⚠️ **This must be fixed before wiring COMPUTED bodies through the query checker (GR-2b), or it fires on all 22 valid corpus sites.**

**(b) Tail path / `[WHERE]` filter loses the type.** `reference_back_traversal_kind` hard-returns `None` unless the idiom is exactly `[graph]` or `[graph, Index]`. So `<~post` → `Array<RecordId<"post">>`, `<~post[0]` → `RecordId`, but `<~post.title`, `<~post[0].title` and `<~post[WHERE title != '']` → `unknown`. A filter narrows rows, not the element kind — and the analyzer already knows this for `->` graphs (`resolve_graph_chain`'s `IdiomPart::Where(_) => continue`, `graph_split`, `tail_plain_segments`). Rewiring to those three existing helpers is the small part.

**(c) The "no REFERENCE points back" diagnostic is computed and thrown away.** `select.rs:943` builds `points_back` and just `return None` when false — indistinguishable from "unprovable". Two guards are mandatory or it FPs: **RELATION edges must be exempt** (`in`/`out` live outside `self.fields`, so `points_back` is false for a valid `<~employee_of`, which currently resolves via the generic-graph fallthrough), and the target must be **SCHEMAFULL**.

**Corpus true positive:** `organization.contacts COMPUTED <~contact` — the `contact` table has *no* `record<organization>` field anywhere in the corpus, and it is the **only** `unknown` among organization's six back-traversals. A genuinely dead COMPUTED field shipping silently in an "all-valid" corpus.

**Touch:** `crates/workspace/src/analyzer/data/graph.rs:225, 374`, `crates/workspace/src/analyzer/data/select.rs:906-960, 1279-1360`, `crates/diagnostics/src/catalog.rs`.

---

### GR-2 — COMPUTED clause bodies are never walked by **any** check
**Effort: medium.**

**Root cause.** `schema/define/field.rs:230` iterates only `[(&stmt.default, true), (&stmt.value, true)]`. `stmt.computed` is a **distinct AST field** (`ast/statement.rs:334`) and is absent. Typing happens on a separate path (`schema.rs:865 infer_field_value_kind`) with a scratch diagnostics sink that is explicitly discarded.

**Proof.**
```surql
DEFINE FIELD v2 ON parent VALUE    fn::does_not_exist(name);  -- error[E5001]
DEFINE FIELD c2 ON parent COMPUTED fn::does_not_exist(name);  -- SILENT
DEFINE FIELD v3 ON parent VALUE    http::get('…');            -- warning[W7012]
DEFINE FIELD c3 ON parent COMPUTED http::get('…');            -- SILENT (and 7012's own doc comment is about computed contexts)
```
So no unknown-function check, no 7012, no graph check, no expression checking of any kind runs inside a COMPUTED body. `<~kid[WHERE nonexistent_col = true]` passes with zero diagnostics.

**Sequencing:** land GR-1(a) first, then wire COMPUTED through.

**Product decision required before shipping.** Enabling E3001 on COMPUTED bodies surfaces 4 findings in the "all-valid" corpus: `payment.refunds COMPUTED <-refund`, `outlet_table_session.orders COMPUTED <-order[…]`, `product.variants COMPUTED <-product[…]`, `organization_product.variants COMPUTED <-organization_product_variant[…]`. I checked each target — refund, order, product, organization_product_variant are all plain SCHEMAFULL tables, none `TYPE RELATION`, so these `<-` steps genuinely return `[]` at runtime. Notably `product.parent` **is** `option<record<product>> REFERENCE`, so the author almost certainly meant `<~product`. **These look like real 2.x-era schema bugs — true positives, not oracle FPs.** Confirm with the maintainer, then fix the corpus rather than the checker.

**Touch:** `crates/workspace/src/analyzer/schema/define/field.rs:230`, `crates/workspace/src/schema.rs:862-889`.

---

### GR-3 — Multi-edge graph step `->(a, b)->target` false-positives E3001 on its landing table
**Effort: small.**

**Root cause, two parts.** (1) `check_step`'s multi-target branch early-returns `StepOutcome { edge_table: None }` — its own comment concedes "not resolving the landing to one table is an analyzer limitation, not a contract violation" — so the *next* step sees `after_edge == false` and treats the plain landing table as a traversal target. (2) That branch **ignores the `after_edge` parameter entirely** and calls `check_edge_is_relation` on every target, so `->tagged->(post, comment)` errors once per landing target.

**Repro.** `SELECT ->(likes, wrote)->post AS p FROM person` → E3001 on `post`. `SELECT ->tagged->(post, comment)` → two E3001s. Controls `->likes->post` and `->(likes, wrote)` alone are clean.

**Also creates a false negative:** `->(likes, wrote)->comment` (unreachable landing) is silently unchecked, because `pending_edge` is `None`. The correct rule is decidable from the schema: after a multi-target edge step where all targets are relations, the landing is reachable iff it's in the union of the edges' far-side tables.

**Note:** the module doc references a code `3005` for "unresolvable multi-target steps" that **does not exist in the catalog** — i.e. the intended handling was a softer finding, never the hard 3001.

**Fix.** `StepOutcome.edge_table` → a set (or an `on_edge` flag); thread it into `check_hop_reachability` for the union check; branch the multi-target arm on `after_edge`. The type-inference half (`single_graph_target` returns `None` for any multi-target step, so the traversal stays untyped) is a separate follow-up.

**Corpus:** zero `->(` occurrences, so no oracle perturbation.

**Touch:** `crates/workspace/src/analyzer/data/graph.rs:95, 225, 374`, `crates/workspace/src/analyzer/data/select.rs:877`.

---

### GR-4 — Graph tail paths use the non-link-crossing resolver
**Effort: small. Three-line swap.**

**Root cause.** `graph_projection_kind` has **both** resolvers reachable and `schema: &SchemaIndex` already in scope, but calls `kind_for_path` (which returns `Kind::Any` when a `record<_>` head has trailing segments) at lines 1329/1355/1359 — while the sibling *destructure* branch at line 1349 in the same match already calls `resolve_field_path`, which crosses links recursively.

**Repro.**
```surql
SELECT ->likes->post.author.name AS an FROM person;  -- Array<unknown>
SELECT ->likes.out.title AS ot FROM person;          -- Array<unknown>
SELECT ->likes->post.author AS a FROM person;        -- Array<RecordId<"person">>   (depth-1 works)
SELECT ->likes->post.{author} AS d FROM person;      -- WORKS (destructure branch already correct)
SELECT on_post.author.name AS deep FROM comment;     -- string  (same 2-hop path, no traversal)
```
**Soundness.** `resolve_field_path` returns `None` only when the head is absent *and* no link was crossed — identical to `kind_for_path` — so the `?` early-returns are unchanged, and `resolve_across_link` already widens to `Any` for `record<>`, schemaless targets and disagreeing unions. Strictly more precise, never inventive.

**Honest impact:** zero corpus occurrences of this shape. Worth doing purely because it's a three-line swap that removes a user-visible inconsistency.

**Separate, out of scope:** `SELECT ->likes->post.{author.name} AS d` yields `unknown` — the destructure branch drops the whole projection when a selected sub-idiom is dotted (likely `plain_field_segments` returning `None`).

**Touch:** `crates/workspace/src/analyzer/data/select.rs:1329, 1355, 1359`.

---

## E. Schema extraction & grammar

### SX-1 — Descendant field definitions overwrite the parent's declared kind
**Merged from:** 3 filings (`field[*]` array drop ×2; missing 1025 / `title.sub` over a string). **Effort: medium. Top-5.**

**Root cause (single line).** `kind_for_path` tests `has_descendants` **first** and unconditionally returns `object_kind_for_field_prefix(...)`, which synthesizes a closed object purely from descendant paths and never reads the parent's own `FieldDef::kind`. The `table.fields.get(...)` branch is unreachable whenever any descendant exists. Compounding it, `idiom_field_path` (`schema.rs:599`) `filter_map`s to **only** `IdiomPart::Field`, so `items[*]` collapses onto `items` and `items[*].sku` onto `items.sku` — even though the parser preserves `IdiomPart::All` (`ast/expr.rs:126-137`).

**Symptoms — all one fix:**

| Schema | Current | Expected |
|---|---|---|
| `items TYPE array` + `items[*].sku/qty` | `items: { qty; sku }` | `items: Array<{ qty; sku }>` |
| `items TYPE array<object>` + `items[*].sku` | declared `array<object>` **discarded** | preserved |
| `optprof TYPE option<object>` + `optprof.name` | `option<>` dropped → `CREATE … SET optprof = NONE` errors E2001 "is not optional" while citing a definition that reads `TYPE option<object>` | optional preserved |
| `title TYPE string` + `title.sub TYPE string` | silently accepted; `CREATE doc SET title='hello'` → **E2001 `title` is declared `{ sub: string }`** | **1025** at the subfield define; parent kind kept |

**Error-severity FPs on valid writes** in the first three rows — `generate` aborts. Reproduces on the oracle: `organization_billing.setup_checkout_line_items` (declared `array` + `[*] object FLEXIBLE` + `[*].price/[*].quantity`) emits `{ price: string; quantity: number }` today.

**Note:** `analyzer/schema/define/field.rs:219-227` already *documents* the collapse — but uses the knowledge only to suppress the E1022 duplicate check, while the stored kind is still clobbered.

**Fix.** (a) Merge instead of replace in `kind_for_path`: refine `object`/literal-object into the closed literal (today's behavior), preserve an `option<>` wrapper, and build the descendant object as the **element** kind under `array<…>`/`set<…>` when the next segment is `*`. (b) Emit **1025** (currently zero emission sites) when the parent's declared kind admits no subfields. Exempt: undeclared parent, `TYPE object`, `TYPE any`/untyped, literal objects, FLEXIBLE, and containers reached via `*`.

**Why medium:** `FieldDef.path` is a `Vec<String>` dotted key threaded through ~68 `.path` sites and 10 `idiom_field_path` call sites; a correct fix needs a segment representation carrying an element step (`Field(String) | AllElements`). `kind_for_path` also feeds SELECT projection, mutation 2001/2004 and codegen.

**Touch:** `crates/workspace/src/analyzer/data/select.rs:1715-1763`, `crates/workspace/src/schema.rs:599`, `crates/workspace/src/analyzer/schema/define/field.rs:219-227`, `crates/diagnostics/src/catalog.rs:44`.

---

### SX-2 — `FOR … IN <iterable>` grammar rejects idioms, function calls and tables
**Effort: medium (grammar + regenerate).**

**Root cause.** `grammar.js:308-322` restricts the iterable to `choice($.Array, $.VariableName, $.Range, $.SubQuery, $._subqueryStatement, $.Block)`.

**Repro (error counts).**
```
0  FOR $x IN [1,2] { … }              0  FOR $x IN ($u.tags) { … }        <- parens fix it
2  FOR $t IN $u.tags { … }            2  FOR $x IN array::distinct([1,2]) { … }  (+ a bogus S0002 "missing node `=`")
2  FOR $u IN user { … }
```
In the embedded-TS path a single occurrence → *"generate failed: 2 error(s) — registry not written"*. On the `.surql` path `generate` **succeeds and writes an out.ts with an empty registry** — silent data loss, arguably worse.

**The whole semantic layer already supports arbitrary iterables.** `lower_for` is generic (`_ if iterable.is_none() => lower_expr(child, text)`); `for_loop.rs` calls `expr_fact` generically and already emits E2022/W7004 correctly. Parenthesizing proves it end-to-end: `FOR $n IN (12) { … }` correctly emits `E2022: FOR can't iterate a 'int'`. Zero analyzer work needed.

**Design constraint.** Widen to a **brace-free value expression** (idiom/field path, function call, method call, index/filter + the existing members) — do **not** swap in `$._value`, and don't let the bare-table form drive the design: both reintroduce the `{`-ambiguity between an object-literal iterable and the loop body, which is almost certainly why the whitelist exists.

**Unconfirmed detail from the filing:** the downstream `W4006 unreachable` artifact did **not** reproduce; treat as unverified. (Category was also misfiled as schema-extraction; this is grammar.)

**Touch:** `crates/tree-sitter-surrealql/grammar.js:308-322` + regenerate `parser.c`.

---

### SX-3 — `ASSERT $value IN [...]` / equality chains never narrow the field kind to a literal union
**Effort: medium. Gated on TI-4.**

**Root cause.** `analyze_define_field` walks `stmt.assert` for diagnostics only and never folds it back into `FieldDef.kind`, which is sourced purely from TYPE. `$value` is already bound during that walk, and `eval_assert` already const-folds assert expressions — the information is entirely in hand.

**Repro.** `DEFINE FIELD role ON account TYPE string ASSERT $value IN ['admin','member']` → `role: string`. A declared `TYPE 'active'|'inactive'` **does** round-trip to `"active" | "inactive"` in TS, so `Kind::Either(Literal…)` is fully supported by codegen.

**Corpus: 118 occurrences across 76 files** — the single highest-leverage source of enum typing in a real schema. Every one is plain `string` in the generated TS today.

**Fold conservatively:** only exact membership/equality enumerations over const literals (`$value IN [lits]`, an OR-chain of `$value = lit`, or such a conjunct of an AND), and **preserve the `option<>` wrapper** since NONE bypasses ASSERT.

**⚠️ Blocking prerequisite: TI-4.** Land literal-assignability on writes in the same change, or all 118 fields inherit the `E2001 'string' vs 'admin'|'member'` FP and every valid write in the corpus becomes an error.

**Touch:** `crates/workspace/src/analyzer/schema/define/field.rs`, `crates/workspace/src/schema.rs:176-202`, `crates/workspace/src/analyzer/flow/narrow.rs` (reuse the existing IN/equality narrowing).

---

### SX-4 — Builtin function registry: whole families missing, and only one spelling per function
**Merged from:** 3 filings (missing families; 2.x/3.x spellings + dead E8001; `not`/`sleep`/`rand` + `rand::duration`). **Effort: medium (a+c) / large (b).**

**(a) ~50 real builtins produce false E5001** (Error/Deny → `check` exits 1, fails CI). Dispatch splits on the first `::` segment then does an exact two-segment `match path`; three- and four-segment names hit `unknown_function`.

Missing, verified present in **both** surrealdb-core 2.x and 3.2.1 unless noted: `vector::distance::{chebyshev,euclidean,hamming,mahalanobis,manhattan,minkowski,knn(3.x)}`, `vector::similarity::{cosine,jaccard,pearson,spearman}`, `string::distance::*`, `string::similarity::*`, `string::semver::{major,minor,patch,compare,inc::*,set::*}`, `geo::hash::{encode,decode}`, `array::sort::{asc,desc}`, `rand::uuid::{v4,v7}`, bare `rand()`, `session::{sc,sd}`(2.x-only), `rand::guid`(2.x-only). 3.2.1 also adds `string::distance::{damerau_levenshtein,normalized_*,osa}` and `string::similarity::{jaro_winkler,sorensen_dice}`.

Every one has a fixed knowable return kind, and `Signature`/`ReturnKind::Fixed`/`SameAsArg` already express what's needed. ~45-50 near-trivial files + dispatch arms; `semver::inc::*` needs depth > 3 handling.

**(b) Only one spelling is known per function.** `normalize_function_path` aliases only `::is::` → `::is_`. So `string::endsWith`/`startsWith` (the **only** 2.x names), `duration::from::days`, `time::from::unix` false-error; conversely 3.x-only names sail through under `surrealdb_version = "2"` because **E8001/E8003 are cataloged with no emission site anywhere** and `surrealdb_version` is parsed but never read by any analyzer.
  - *Half A* (alias legacy→canonical) is **small** and semantics-preserving (3.2.1 literally maps `duration::from_days => duration::from::days`).
  - *Half B* (implement E8001) is **large** and carries a product decision: the corpus **pins `"2"` but uses only 3.x spellings** (`string::starts_with` ×6, `string::is_email` ×3, `is_uuid`, `is_domain`, `duration::days`, `duration::years`). Implementing E8001 naively lights up the zero-FP oracle with ~13+ new errors. **The corpus pin and/or the `"2"` default is what's stale — fix that in the same change.**
  - `array::sort::asc`/`desc` are **not** a version issue: present in both registries, no fallback spelling, and they need real analyzer entries, not a normalization rule.

**(c) Unqualified builtins.** `grammar.js:1914-1921` allows only `rand` and `count` unqualified (`FunctionName` requires a `::` or an `fn` prefix). The authoritative set from 3.2.1 is exactly `count, not, rand, sleep` — so `not(true)` and `sleep(1s)` are **S0001 syntax errors** while `function/not/not.rs` and `function/sleep/sleep.rs` sit as **unreachable dead code with passing unit tests and signatures that match upstream byte-for-byte**. `RETURN NOT true` (prefix operator) also fails to parse. Bare `rand()` parses but has no dispatch arm → E5001. And `rand::duration`'s signature is exactly **inverted**: `ParamKind::Numeric` where upstream is `(?min: Duration, ?max: Duration)` — so `rand::duration(1s,2s)` errors and `rand::duration(1,2)` passes. That last one is a **one-line fix**; do it today.

**Corpus:** none of these names appear, so no oracle regression — a latent FP wave that fires the moment a user writes a vector-search, geohash or semver query.

**Touch:** `crates/workspace/src/analyzer/function/mod.rs:42`, `…/function/{vector,string,geo,rand,array,session}/mod.rs`, `…/function/rand/duration.rs`, `crates/syntax/src/lower/expr.rs:633`, `crates/tree-sitter-surrealql/grammar.js:1914`, `crates/workspace/src/config.rs:38-41`, `crates/diagnostics/src/catalog.rs:138-139`.

---

### SX-5 — `DEFINE PARAM … VALUE <expr>` stores no kind, so cross-file `$PARAM` reads are `unknown` **and** are demanded as caller bindings
**Effort: medium.**

**Root cause, two independent links.** (1) `ParamDef` has no `kind` field, so nothing survives into the workspace schema and `analyze_define_param`'s `ctx.define_param_default` is session-local. (2) `param_fact` checks only `env.let_fact(name)` and **never consults `env.param_default_fact(name)`** — which exists and is already used by `flow/let_stmt.rs:58` and `record_param_use`. So the read is `unknown` even *in the same file*.

**Repro.**
```
"RETURN $LIMIT;"                    -> { result: [unknown]; params: { LIMIT: unknown } }
"RETURN string::uppercase($LIMIT);" -> { result: [string];  params: { LIMIT: string } }   <- demands a *string* binding for a DB int
"DEFINE PARAM $X VALUE 5; RETURN $X;" -> { result: [null, unknown]; params: { X?: number } }  <- binding typed, read still unknown
```
A schema-defined param is **indistinguishable from an unknown host param**, so every call site is forced to pass a value the database already supplies.

**The module's own contract doc contradicts the behavior** (`define/param.rs:3-5`): "the definition gives `$name` a database-side default, so later reads are neither unknown nor host-required — they carry the default's kind unless the host overrides." Both properties fail.

**Corpus:** 8 params (`$APP`, `$STRIPE_SECRET`, `$PLATFORMS`, `$TIMEZONES`, `$COUNTRY_CALLING_CODES`, …) consumed from *other* files — `device_key.surql:17 ASSERT $value IN $PLATFORMS`, 6× `IN $TIMEZONES`, `phone_number.surql:8`, `@api/organization/create.surql:87`. All checked against an untyped param.

**Secondary (falls out of the same fix):** a use contradicting the declared default emits no 6001/2004.

**Touch:** `crates/workspace/src/schema.rs:33-44, 735-742` (note: `ParamDef` is Serialize/Deserialize — part of the cached schema), `crates/workspace/src/analyzer/schema/define/param.rs`, `crates/workspace/src/analyzer/statement_env.rs:181`, `crates/workspace/src/analyzer/expression/infer.rs:247-259`.

---

### SX-6 — `DEFINE TABLE … AS SELECT` (views) derive no fields
**Effort: medium. Marginal by corpus impact; strong by principle.**

**Root cause.** `DefineTable` has no view field; `lower/statement.rs:1114` explicitly drops `TableViewClause` ("Recognized but not modeled for type inference"). The grammar node is fully structured (`grammar.js:1345`: projections, FROM, WHERE, GROUP).

**Repro.** `DEFINE TABLE stats AS SELECT city, count() AS total, math::mean(age) AS avg_age FROM user GROUP BY city;` → `SELECT * FROM stats` is `unknown`, and `SELECT completely_bogus_field FROM stats` produces **no E1002** (registered as an empty schemaless table).

**Soundness is proven by the analyzer itself:** running the identical projection as a standalone query yields exactly `Array<{ avg_age: number; city: string; total: number }>`. The shape is already computed; it's never applied to the view table.

**Effort drivers:** the view projection is `_inclusivePredicate` under a distinct node kind (SELECT lowering only partly reusable); needs a schema pre-pass with dependency ordering (views on views); should merge any explicit `DEFINE FIELD … ON <view>` on top.

**Honest priority:** **zero** `AS SELECT` occurrences in the corpus. Do after the FP wave.

**Touch:** `crates/syntax/src/ast/statement.rs:284-301`, `crates/syntax/src/lower/statement.rs:1114`, `crates/workspace/src/analyzer/schema/define/table.rs:11-47`, `crates/workspace/src/analyzer/pipeline.rs:115-245`.

---

### SX-7 — Parameterized type constructors beyond `record`/`array`/`set` are unsupported
**Effort: small (geometry) / medium (references) / separate (sequences).**

**Root cause.** `parameterized_kind` handles only `record`, `array`, `set`; everything else is `Err(unsupported())` → `FieldDef.kind = None` → `unknown`. `base_kind_for_name` has no `references` arm.

**Repro.** `geometry<point>`, `geometry<polygon | multipolygon>`, `references<post>`, bare `references` → **all `unknown`**, i.e. strictly *worse* than bare `geometry` (which resolves to `GeoJSON`).

**Correction to the filing:** it is **not** silent — **H6003** *is* emitted per field with correct spans and a covering unit test. It's invisible only because the catalog registers 6003 as `Severity::Hint` + `Allow`. So this is a *recorded known limitation*, and its invisibility is compounded by **DX-8** (`--json` drops warnings). Still a real gap: the type is trivially expressible upstream.

**Split the work:**
- **`geometry<…>` — small, do it.** `Kind::Geometry(Vec<GeometryKind>)` exists in the pinned surrealdb-types 3.1.3 with all 7 variants; the grammar already parses the parameterized, nested and option-wrapped forms. One match arm. **But:** the immediate win is `unknown → GeoJSON`, not Point-shaped GeoJSON — `codegen/src/lib.rs:50` is `Kind::Geometry(_) => "GeoJSON"` and the client type has no parameter. Per-variant narrowing is a separate client-type change.
- **`references<T>` — medium.** Model as `Array<Record([T])>` (bare → `Array<Record([])>`), but the two-arg form `references<post, author>` is a **hard S0001**, so it needs grammar work too.
- **`DEFINE SEQUENCE seq1 BATCH 1000 START 100;` — file separately.** Hard S0001; SurrealDB 3.0 schemas using sequences cannot be analyzed at all. Grammar gap, unrelated to type constructors.

**Corpus:** zero geometry, zero references, zero sequences — impact is entirely for geospatial/reference users.

**Touch:** `crates/workspace/src/schema.rs:987-1038, 1064-1090`, `crates/tree-sitter-surrealql/grammar.js`, `crates/codegen/src/lib.rs:50`.

---

## F. Marginal, deferred, or genuinely unknowable

**Do these last, or not at all — listed so nobody re-derives them.**

| Item | Verdict |
|---|---|
| **TI-10** untyped recursive UDF fixpoint | Real but resolves **zero** corpus cases; the cited corpus hit was misattributed. Medium effort for a synthetic-only win. |
| **TG-8** `FROM (SELECT …)` projections | Zero corpus occurrences. Do the two-line "keep the `Array<>` wrapper" mitigation now; defer the object-literal projection path. |
| **GR-4** graph tail link-crossing | Zero corpus occurrences, but it's a 3-line swap — take it. |
| **DX-12** 5010 event cycles | Large, needs an EventDef store (DX-10 dependency) plus op-aware edges and the guardedness carve-out, for zero current true positives. Sequence last. |
| **SX-6** view tables | Zero corpus occurrences. Principled but not urgent. |
| **`.*` object projections** (`SELECT obj.*`) | Deliberately **not** bundled with TG-6. SurrealDB's `.*` on an object yields the object's *values*, not its members — the proposed expected output is probably wrong. Needs a semantics decision before any code. |
| **E8001 version gating** (SX-4b) | Large + a product decision. Fix the stale corpus pin / `"2"` default first, or it manufactures an FP wave in the very corpus used to prove there are none. |

**Genuinely unknowable — do not file again:**

- **JS `function(){}` closure return kinds.** `organization/@functions.surql:44 LET $priority = function($source) {…}` is a JavaScript block, not a SurrealQL closure. It stays `unknown` after TI-9(b) and correctly so. It was cited as impact for TI-9 and should not have been.
- **`$auth` under JWT / `AUTHENTICATE` access (`DEFINE ACCESS`).** For a *pure* record-access schema the table set is fully determined and `option<record<T1|T2>>` is inferable — so the mechanism is legitimate. **But the naive union rule regresses the oracle:** the corpus's live account access is `TYPE JWT` (the record version is commented out), the only live `TYPE RECORD` access returns `organization_invitation`, and its 7 `$auth.<field>` sites read `account` fields. Inferring the union would produce 7 false E1002s and risk conflict findings on 81 bare `$auth` comparisons. The corpus's real binding comes from an external IdP's JWT claim — genuinely unknowable from SurrealQL. Any implementation **must** fall back to open `record<>` whenever any `TYPE JWT`, `WITH JWT`, or `AUTHENTICATE` clause exists. Large effort, zero oracle gain. **Severity: low.**
- **`Kind::Range` in generated TS.** Codegen deliberately maps it to `"unknown"`; fixing TI-8 changes diagnostics only until the client gains a `Range` type. Not a bug in inference.

---

## Suggested sequencing

1. **FP wave** (unblocks codegen for real users): TI-1, DX-1, SX-1, DX-2, DX-3, TI-4, GR-1(a), GR-3, SX-2, SX-4(a,c), TI-5. Plus DX-8 (`--json` warnings) — it hides the evidence for everything else.
2. **Row-shape correctness**: TG-1, TG-2, TG-7, TG-3.
3. **Path resolution precision**: TI-2, TG-4, TG-5, TG-6, GR-4, TI-3 (do TI-3 first, it's a one-liner).
4. **Contract coverage**: DX-4, DX-5, DX-6, DX-7, SX-3 (+TI-4), DX-9.
5. **Registry / long tail**: SX-4(b), SX-5, DX-10, DX-11, SX-7, GR-2 (after the GR-1 product decision), DX-12.

⚠️ **Two corpus hygiene items first.** `check --json` on `/Users/drewridley/Documents/Projects/workshop/database` currently reports **37 errors / 40 diagnostics**, rooted in an S0001 at `schema/organization/employee_of.surql:1` — the file's first line literally reads `is DEFINE TABLE OVERWRITE employee_of SCHEMAFULL` (stray leading `is `). The oracle appears to have been corrupted outside the analyzer. **Restore the zero-finding baseline before mining it further**, and separately decide whether the corpus's `surrealdb_version = "2"` pin (contradicted by its own 3.x-only function spellings) should become `"3"`.
---

## Addendum — gaps found while implementing the FP wave (2026-07-25)

### NEW-1 — Array-form `INSERT` payloads are dropped by the lowerer, so they are entirely unchecked
**Severity: high. Effort: small–medium.** Found by the DX-3 lane, independently verified.

`INSERT INTO t [{ … }, { … }]` lowers to `InsertData::Values([])` — the lowerer discards
array payloads, so the `Expr::Array` arm in `check_insert_payload` is unreachable dead code.
Consequence: **no** payload checking at all for the array form — not required fields, not
unknown field names, not value kinds.

Repro (schema: `person` SCHEMAFULL with required `name: string`, `age: int`):
```surql
INSERT INTO person { name: 'a' };                      -- E2034 `age` must be set   (correct)
INSERT INTO person [{ name: 'a' }];                    -- SILENT (should be E2034)
INSERT INTO person [{ bogus_field: 1, another: 2 }];   -- SILENT (should flag both)
```
Observed: exactly one diagnostic across all three statements.

Fix requires `crates/syntax/src/lower/statement.rs` (preserve the array payload in the lowered
AST), then the existing `check_insert_payload` array arm becomes live. Note the backlog's
DX-3 premise that "insert.rs already destructures that same array" is **wrong** — the
destructuring exists but can never run.

### NEW-2 — `SELECT -x FROM t` (prefix negation in a projection) is a syntax error
**Severity: low. Effort: small (grammar).** `!x` parses in projection position but `-x` raises
S0001. Grammar gap in the projection expression rule.

### NEW-3 — `cargo fmt --check` is dirty at baseline
~200 pre-existing formatting diffs across all crates. Worth a single formatting commit so
future changes can be fmt-gated in CI.

### NEW-4 — `FOR` over an `option<array<T>>`/`Either` iterable leaves the loop variable untyped
**Severity: medium. Effort: small.** Surfaced by the TI-5 lane, verified independently.

`analyzer/flow/for_loop.rs` computes the element kind with a *flat* match:
```rust
Some(Kind::Array(element, _) | Kind::Set(element, _)) => Some((**element).clone()),
_ => None,
```
An `option<array<string>>` is `Either([None, Array(String)])`, matches neither arm, and the
loop variable falls back to untyped — so nothing inside the body is checked against it.

Repro (schema: `tags` is `option<array<string>>`):
```surql
LET $c = ['a','b'];                       FOR $t IN $c { RETURN $t.not_a_real_method(); };  -- E5001 (correct)
FOR $t IN (SELECT VALUE tags FROM ONLY t LIMIT 1) { RETURN $t.not_a_real_method(); };       -- SILENT
```
Observed: the plain-array control errors; the `option<array>` form produces nothing.

Fix: migrate this site onto the `collection_element_kind` helper added in `expression/infer.rs`
by the TI-5 fix, which already unions the element kind across every collection arm.

### NEW-5 — Subscript element kind on a mixed union takes only the first arm
**Severity: low–medium. Effort: small.** Source-verified (`crates/workspace/src/query.rs`).

`element_kind` resolves an `Either` with `.find_map(element_kind)` — first arm that has an
element kind wins, so `array<int> | array<string>` subscripts to `int` and silently drops
`string`. This is the LSP-facing path (hover/subscript), so it reports a confidently *wrong*
type rather than a loose one.

Fix: same migration as NEW-4 — union across arms instead of `find_map`.

---

### NEW-6 — A wildcard under `GROUP BY` fabricates every non-grouped field
**Severity: high (wrong, not loose). Effort: small–medium.** Found while verifying TG-1,
independently confirmed.

`SELECT * … GROUP BY k` types the row from **every declared field**, but a grouped row only
carries the group keys plus whatever accumulators the projection asks for. The non-grouped
fields do not exist at runtime, so the emitted type is confidently **wrong** — worse than
`unknown`, because consumers dereference fields that will be undefined.

Repro (`emp` has `dept`, `salary`, `nickname`):
```surql
SELECT * FROM emp GROUP BY dept;
```
Observed: `Array<{ dept: string; nickname: string; salary: number }>`
Expected: `Array<{ dept: string }>` (group keys only; a wildcard adds no accumulators).

Note this is *pre-existing*, not caused by TG-1 — TG-1 correctly excluded grouped rows from
the implicit `id` for exactly this reason (a grouped row is synthesized, not materialized) and
deliberately did not stack a new claim on top of the existing one.

### NEW-7 — A non-key, non-aggregate field under `GROUP` is typed as a scalar, but 3.x returns an array
**Severity: medium. Effort: small–medium.** Found while fixing NEW-6, verified against a live
SurrealDB 3.0.5 server and the 2.x core source.

```surql
SELECT dept, salary FROM emp GROUP BY dept;
```
3.x returns `{dept: 'a', salary: [10, 20]}` — the non-grouped field collapses to the **array** of
that group's values. 2.x takes the first value. The analyzer types `salary` as the scalar in both
cases, so under the 3.x target it is a wrong type, not a loose one. Version-gated behavior:
`analysis.surrealdb_version` already exists in the config.

### NEW-8 — `SELECT * … GROUP ALL` has no diagnostic
**Severity: medium. Effort: small.** SurrealDB 3.x **rejects** a wildcard under any GROUP clause
outright (`Incorrect selector for aggregate selection, expression `*` … cannot be aggregated in a
group`), and 2.x silently drops it — so the query never does what its author intended under either
engine. After NEW-6, `GROUP BY k` at least raises `W4013` ("project the key"), but the `GROUP ALL`
form raises nothing at all.

The contract-correct response is a dedicated error-severity code for "wildcard under GROUP", not
just a better inferred type. That needs a new entry in `crates/diagnostics/src/catalog.rs` plus the
catalog markdown, so the NEW-6 lane (scoped to `analyzer/data/`) deliberately left it.

### NEW-9 — Unaliased projection keys for calls/methods don't match the engine
**Severity: high (wrong key — consumers index `undefined`). Effort: small.**
Verified against a live SurrealDB 3.0.5.

`unaliased_computed_key` (`analyzer/data/select.rs`) special-cases bare `count()` and otherwise
uses the **raw source text** of the expression. The engine names these projections differently:

| expression | SurrealDB key | ours | ok |
|---|---|---|---|
| `string::len(name)` | `string::len` | `string::len(name)` | wrong |
| `time::now()` | `time::now` | `time::now()` | wrong |
| `fn::abc(age)` | `fn::abc` | `fn::abc(age)` | wrong |
| `name.len()` | `name` | `name.len()` | wrong |
| `count()` | `count` | `count` | ok |
| `age + 1` | `age + 1` | `age + 1` | ok |
| `math::abs(age) + 1` | `math::abs(age) + 1` | same | ok |
| `age > 20` | `age > 20` | same | ok |

**Rule the engine follows:** if the top-level expression *is* a function call, the key is the bare
function name (no parens, no arguments). If it is an idiom ending in a method call, the key is the
idiom with the method dropped (`name.len()` -> `name`). Otherwise the key is the source text.
Note the rule is about the *top-level* node: `math::abs(age) + 1` is a binary expression, so it
keeps its source text even though it contains a call.

This is a *wrong key*, not a loose type — generated TS declares `"fn::abc(age)"` while the runtime
returns `fn::abc`, so every consumer of an unaliased call projection reads `undefined`.

### NEW-10 — `name.len()` (method on a string field) infers `unknown`
**Severity: medium. Effort: small.** Found alongside NEW-9.

`SELECT name.len() FROM person` types as `unknown`; the engine returns an int (verified: `1` for
`name = 'A'`). The `string::len` *function* resolves correctly, so this is the method-dispatch path
(`method_return_kind`) failing to map `.len()` on a `string` receiver. Worth auditing the whole
method-dispatch table against the builtin registry, since one missing entry implies others.

## Status — FP wave complete (2026-07-25)

Fixed and merged: **TI-1** (`??` NONE-strip), **TI-4** (literal-union writes, plus the exact-literal
check restored at the write site), **TI-5** (`Either` over index/method), **SX-4** (~60 builtins),
**DX-1** (GROUP/ORDER alias), **DX-2** (aggregate promotion), **DX-3** (opaque `CONTENT` payload),
and two CLI integrity bugs (`check` now scans host files; `--json` keeps warnings on a clean run).

Measured on the audit's own repros: valid input went **12 error-severity false positives → 0**,
while every genuinely-invalid case still reports. Oracle unchanged at 40; 779 tests.

Still open and highest-value: **TG-1** (implicit `id`/`in`/`out`), **TI-2** (`option`/`array`
record-link traversal), **SX-1** (descendant field defs clobbering the parent kind) — the
"row shape" cluster, which rewrites type expectations broadly and should be done serially.
