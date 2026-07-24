# `Kind::Any` audit — analyzer / inference

**Scope:** `crates/workspace/src`, `crates/syntax/src`, `crates/codegen/src`
**Branch/commit:** `redesign-v3-foundation` @ `2823a4e`
**Mode:** read-only audit; no code changed.

## TL;DR

The headline finding is that **the clusters the audit expected to be the
biggest wins are already implemented.** Higher-order closure typing
(`array/set::map|filter|fold|reduce`), `object::values`, `record::id` /
`meta::id`, `record::table`, record-link projection traversal, and the `.{}`
destructure were all migrated already. What's left in the `Kind::Any` census
is overwhelmingly *legitimate*: argument contracts (`ParamKind::Any`), the
"any is compatible" logic in checking, rendering, poison-after-error, and
partial-propagation degradation.

The genuine remaining inference gaps are a **short** list — 6 clusters, mostly
S/M effort with modest impact. Closure-return typing is **not** #1; it's done.

- Total `Kind::Any` matches (workspace+syntax+codegen): **335**
- Test-body / `assert_eq!` expectations excluded: **68**
- **Production occurrences audited: 267**
- `crates/syntax/src`: **0** occurrences (the "few in syntax" the brief
  anticipated do not exist on this commit).
- `crates/codegen/src`: **1** production occurrence (a renderer; legitimate).

---

## (a) Summary tables

### By bucket (production, 267)

Pattern buckets overlap slightly (a line can be both a doc-comment and code);
counts are indicative, and every **FIXABLE** line is enumerated exhaustively in
section (b).

| Bucket | ~Count | Notes |
|---|---:|---|
| LEGITIMATE — genuinely-any *argument contract* (`ParamKind::Any`) | ~98 | one per `type::is_*`, `count::count`, `type::of`, `record::id`, etc. Input slot that accepts anything. |
| LEGITIMATE — guard / assignability / unification logic | ~99 | `== Kind::Any`, `matches!(…, Kind::Any)`, `Kind::Any => true`, `(Kind::Any, other) => …`. Not fallbacks — this is the "any is compatible with everything" rule. |
| LEGITIMATE — genuinely-any *return contract* (`Fixed(Any)` / `array<any>`) | ~14 | `record::id`, `meta::id`, `record::refs`, `type::array`, `encoding::cbor_decode`, `object::entries`, `array::combine`, http bodies, json/cbor decode, etc. |
| LEGITIMATE — poison-after-error / parse-failure fallback | ~30 | `return Kind::Any` after emitting an E-code, `Partial(_) => Kind::Any`, unresolved-target returns. |
| LEGITIMATE — degradation propagation (`.unwrap_or(Kind::Any)`) | ~30 | a sub-expression was genuinely partial/unresolved; the Any is inherited, not invented. Not independently fixable. |
| LEGITIMATE — missing/wrong-shape arg fallback (`_ => Kind::Any`) | ~30 | closure arg absent, first arg not an array/set, empty call path. |
| **FIXABLE — hidden inference gap** | **6 clusters (~7 lines)** | section (b). |

### By file (production count, highest first)

| File | Prod | Character |
|---|---:|---|
| `analyzer/data/select.rs` | 32 | guards + poison-after-1002 + `unwrap_or` + record-link resolvers (mostly legit; 1 fixable-adjacent, see F2) |
| `analyzer/expression/check.rs` | 13 | **all** guards / "any is assignable" — zero fallbacks |
| `analyzer/data/mutation.rs` | 12 | guards + poison-after-error + RETURN-shape (1 minor candidate, F6) |
| `analyzer/function/signature.rs` | 11 | `ParamKind`/`ReturnKind` machinery; `evaluate` returns Any only when the source arg is missing/non-collection |
| `analyzer/expression/infer.rs` | 11 | degradation + closure param defaults; **1 fixable (F1, mixed array)** |
| `analyzer/function/mod.rs` | 7 | empty-path, unknown-fn poison, **UDF untyped-return (F3)** |
| `kinds.rs` | 4 | assignability guards |
| `schema.rs` | 4 | type-expr parsing (`any` keyword → `Kind::Any`), `array<>`/`set<>` element default |
| `analyzer/function/{array,set}/fold.rs` | 4 ea | closure-missing / non-collection fallbacks (closure body **is** typed) |
| `analyzer/flow/if_else.rs` | 3 | condition degradation + empty-branch fallback + not-bool guard — no gap |
| `analyzer/function/type_/field.rs`, `type_/fields.rs` | 3 | field-path resolution implemented; Any on non-const / unknown path |
| `analyzer/function/object/values.rs` | 3 | **implemented** (union of field kinds); Any only for empty/bare/other |
| `analyzer/function/{array,set}/reduce.rs`, `.../map.rs` | 2–3 | implemented via `closure_return_kind` |
| ~130 single-occurrence function files | 1 ea | `ParamKind::Any` arg slot, `Fixed(Bool)` is_* guards, cast returns, or non-array fallback |
| `codegen/src/lib.rs` | 1 | `Kind::Any => "unknown"` renderer |

---

## (b) Ranked FIXABLE list (highest impact first)

### F1 — Mixed-element array literals punt to `array<any>`
- **Where:** `crates/workspace/src/analyzer/expression/infer.rs:773`
  (`Box::new(element_kind.clone().unwrap_or(Kind::Any))`, with the
  `mixed array` partial pushed at `:781`).
- **Construct:** array literal inference (`array_fact`). `homogeneous(...)`
  returns `None` when elements disagree, so `[1, "a"]` infers
  `array<any>` + an `UnsupportedSyntax("mixed array")` partial.
- **Correct kind:** `array<int | string>` — the `Kind::either` of the element
  facts (drop the partial). Empty-array `unwrap_or(Any)` stays.
- **Effort / impact:** **M / M.** Direct win for object literals and inline
  arrays in CONTENT/RETURN; feeds every downstream consumer of array element
  typing (indexing, `.map`, projections).

### F2 — `kind_for_path` leaves record-link paths opaque for non-projection callers
- **Where:** `crates/workspace/src/analyzer/data/select.rs:1521`
  (`Some(Kind::Record(_)) => Some(Kind::Any)` inside `kind_for_path`).
- **Construct:** the *base* resolver stops at a record-link boundary and yields
  `Any` for the trailing segments. The link-crossing resolver
  `resolve_field_path` (`select.rs:1536`, added in **2823a4e**) does the right
  thing and is already wired into SELECT projections (`:838`, `:957`, `:1144`,
  `:1185`, `:1257`) — **verified resolved for projections; do not relist.**
  The gap is the *other* callers of `kind_for_path` that can receive a
  multi-segment cross-link path:
  - `query.rs:1041`, `:1103`, `:1544` (hover / field-path typing)
  - `select.rs:283`, `:314` (WHERE / GROUP field filters — `WHERE friend.name = …`)
- **Correct kind:** cross the link (as `resolve_field_path` does) instead of
  `Any`. Migrate the cross-link-capable callers, or teach `kind_for_path` to
  delegate.
- **Effort / impact:** **M / M.** Improves hover and WHERE/GROUP precision on
  linked fields. (`infer.rs:317/435/495` call it with a *single* segment, so
  they never hit `:1521` — leave them.) `resolve_across_link`'s own Any returns
  (`:1581/1586/1589/1594`) are legitimate: empty `record<>`, unknown target, or
  variants that disagree.

### F3 — User-defined functions without a declared return type infer to `Any`
- **Where:** `crates/workspace/src/analyzer/function/mod.rs:93`
  (`function.return_kind.clone().unwrap_or(Kind::Any)`); the body kind is
  already computed at define time in
  `crates/workspace/src/analyzer/schema/define/function.rs:27`
  (`.unwrap_or(Kind::Any)`), used only to *check* against a declared return.
- **Construct:** `DEFINE FUNCTION fn::foo(...) { ... }` with no `-> T`. Calls
  to it type as `Any` even though the body's kind is inferable (and is inferred
  transiently for the assignability check).
- **Correct kind:** the inferred body kind — persist it on the schema's
  `FunctionDef.return_kind` when the declaration omits an explicit return, then
  call sites reuse it.
- **Effort / impact:** **L / M.** Requires storing the inferred return on the
  schema entry (architectural touch). High value for codebases that lean on
  untyped `fn::` helpers.

### F4 — `object::entries` on a closed object literal drops the value kind
- **Where:** `crates/workspace/src/analyzer/function/object/entries.rs:22–25`
  (`Fixed(Array(Array(any)))`).
- **Construct:** `object::entries({...})` where the arg is a
  `Kind::Literal(Object(...))`. Always returns `array<array<any>>`.
- **Correct kind:** each pair is `[string, V]` where `V` is the union of the
  object's field kinds. `Kind` can't express a heterogeneous 2-tuple, so the
  realistic improvement is `array<array<string | V1 | V2 | …>>`. Requires
  leaving the `Signature` table and writing a body like `object::values`.
- **Effort / impact:** **M / Low.** Marginal — the tuple shape isn't
  faithfully representable.

### F5 — `array::combine` pair element is `any`
- **Where:** `crates/workspace/src/analyzer/function/array/combine.rs:24–25`
  (`Fixed(Array(Array(any)))`).
- **Construct:** `array::combine(a, b)` returns pairs drawn from the two input
  arrays. Element could be `array<A | B>` from the two element kinds.
- **Effort / impact:** **S / Low.** Same tuple-representability caveat as F4.

### F6 — Schemaless-table full-row response could be `object`, not `any`
- **Where:** `crates/workspace/src/analyzer/data/mutation.rs:664`
  (`if table.fields.is_empty() { return Kind::Any; }` for BEFORE/AFTER/default
  RETURN).
- **Construct:** CREATE/UPDATE/etc. returning full rows of a table with no
  declared fields. The row is genuinely arbitrary, but it is always an
  *object*, so `Kind::Object` is strictly more precise than `Kind::Any`.
- **Effort / impact:** **S / Low.** Debatable whether worth the churn; listed
  for completeness. (The parallel `select.rs` full-field path already produces
  an object when fields exist.)

**Not fixable but worth a design note:** the dominant *upstream source* of Any
is an untyped schema field — `schema/define/field.rs:96`
(`declared.clone().unwrap_or(Kind::Any)`) and `select.rs:1502`
(`field.kind.clone().unwrap_or(Kind::Any)`). A field defined without a `TYPE`
clause is honestly `any`; these are correct. Many downstream Anys simply
inherit this and are not independent gaps.

---

## (c) Legitimate — leave alone (appendix, so we don't re-audit)

### Genuinely-any *contract* (input or output truly untypeable)
- **`type::is_*` guards** (`type_/is_array.rs`, `is_bool`, `is_record`, … 27
  files): `ParamKind::Any` input, `Fixed(Bool)` return. Correct.
- **`type::field` / `type::fields`** (`type_/field.rs:33,57,75`,
  `type_/fields.rs:21,27,31`): field-path resolution **is** implemented; Any is
  returned only for a non-constant path, a provably-non-string arg (after
  emitting 5005), or an unknown field (after 5005). Correct.
- **`record::id` / `meta::id`** (`record/id.rs:25`, `meta/id.rs:26`):
  `Fixed(Any)`. **Audit hypothesis denied** — `surrealdb_types::Kind` has no
  "record-id kind"; a record id can be string/int/uuid/array/object/range, so
  `Any` is the honest answer, per the file docs.
- **`record::refs`** (`record/refs.rs:25`): cross-table result → `array<any>`.
- **`object::from_entries`** (`object/from_entries.rs:22`): dynamic keys →
  `Kind::Object` (already better than Any).
- **`type::array`** (`type_/array.rs:21`), **`array::combine`/`object::entries`**
  (see F4/F5), **`encoding::cbor_decode`** (`Fixed(Any)`), **`encoding::*_encode`**,
  **`http::post|put|patch`** response bodies, **`file::*`**, **`api/*`**,
  **`value::diff`** (`array<object>`): opaque by contract.
- **`<any>` cast**, **`json_value_kind()`** (`function/mod.rs:152` — the
  `array<any>` inside the JSON union is correct, decoded elements are opaque).
- **Empty call path** (`function/mod.rs:52`): param-invocation of a closure
  variable; no name to resolve.
- **`ReturnKind::SameAsArg` / `ArrayElement` misses** (`signature.rs:267,270`):
  `Any` only when the referenced arg is absent or non-collection — an invariant
  violation, not ambiguity. Correct by design (documented at `signature.rs:9`).

### Poison-after-error (Any inserted right after an E-code)
- `select.rs:146,862,968,979` (after `check_field_path` / E1002).
- `mutation.rs:730` (after `check_field_path` 1002).
- `function/mod.rs:117,143` (after 5001 unknown-function).
- `create.rs:35`, `update.rs:34`, `delete.rs:34`, `upsert.rs:37`,
  `relate.rs:55`, `insert.rs:30` (after `check_table_reference`).

### Parse / lower failure (unparseable node)
- `analyzer/statement.rs:55` (`Statement::Partial(_) => Any`).
- `schema/define/mod.rs:30` (`DefineStmt::Other(_) => Any`).
- `mutation.rs:706`, `select.rs` partial projections (keyed by source text).
- `infer.rs` `partial_fact` / `PartialReason::*` paths.

### Missing / wrong-shape argument fallback
- `array/set::map|fold|reduce` (`return Kind::Any` when the closure arg is
  absent or arg0 is not a collection) — the closure body **is** typed via
  `closure_return_kind` (`infer.rs:133`). Verified implemented.
- `array/set::filter` — `_ => Any` when arg0 isn't a collection; filtering
  preserves the input element kind otherwise. Correct.
- `array::flatten|group|windows|clump|repeat`, `set::flatten` — `_ => Any`
  only when arg0 isn't an array/set; the array case is typed. Correct.
- `rand::enum` (`rand/enum.rs:17`) — `Any` only for zero args; otherwise
  `either(args)`. Correct.
- `create/update/delete/upsert/relate/insert` first `return Kind::Any` — the
  target table name didn't resolve (dynamic/unparseable target). Correct.

### Degradation propagation (`.unwrap_or(Kind::Any)` inheriting an upstream partial)
- `infer.rs:92,95,113,145,347,457,801` (RETURN/Expr value, closure param
  defaults, method/call arg kinds).
- `expression/mod.rs:27`, `context_params.rs:112`,
  `mutation.rs:742`, `select.rs:1006,1144,1185,1257,1480,1502`.
- `if_else.rs:31` (condition kind, used only for the not-bool warning),
  `if_else.rs:74` (empty IF with no branches/else).

### Guards, unification, rendering (not fallbacks at all)
- **`expression/check.rs` — every occurrence** (`:211,216,276,299,635,655,
  701,702,747,790,791,792,874`): "an `any` operand/receiver/element is
  compatible" rules. No result-type punt here.
- `kinds.rs:32,39,96,97` (assignability), `statement_env.rs:287`
  (`Any` absorbs in unification), `mutation.rs:121,467,523,611`,
  `select.rs:258,284,316,451,479,673`, `graph.rs:51,194`, `kill.rs:15`,
  `insert.rs:110`, `if_else.rs:83`, `function/mod.rs:229,239`,
  `schema/define/field.rs:45`, `schema/define/function.rs:40` (all guards).
- `query.rs:50,430` and `codegen/src/lib.rs:23` (render `Kind::Any` →
  `"any"` / `"unknown"`).
- `schema.rs:841,890,904,905` (type-expr parser: the `any` keyword maps to
  `Kind::Any`; `array<>`/`set<>` default element before the inner type is read).

---

## Verification notes for the just-landed fix (2823a4e)

- **Record-link projection traversal:** RESOLVED. `resolve_field_path`
  (`select.rs:1536`) crosses links recursively and is used by every SELECT
  projection path. The residual `Any` in `kind_for_path:1521` is the *base*
  resolver's intentional boundary; its only remaining impact is on non-
  projection callers (hover / WHERE) — captured as **F2**, not relisted as the
  projection gap.
- **`.{}` destructure:** RESOLVED. `step_part_kind` /`step_idiom_kind`
  (`infer.rs:353,463`) build a closed `Kind::Literal(Object)` from the selected
  sub-paths via `field_of_kind`; returns `None` (not `Any`) on an unknown
  sub-field, which surfaces as the correct diagnostic rather than a poison type.
