# The expression-fact layer

Status: design / proposal
Author: SurrealGuard principal engineering
Date: 2026-07-26
Supersedes: nothing. Replaces the recognizer set in `crates/workspace/src/analyzer/flow/narrow.rs`
and the shape-matching in its sibling consumers.
Depends on: the flow engine (`analyzer/flow/`), the quality harness
(`crates/workspace/tests/precision_snapshot.rs`, `tests/any_ratchet.rs`).

---

## 0. The one-paragraph version

Narrowing today is thirteen hand-written recognizers, each of which accepts one exact
syntactic shape and silently declines every equivalent spelling of the same fact. The same
style is repeated across **~189 distinct AST-shape match sites** in the analyzer — 60 in
`data/select.rs`, 34 in `expression/infer.rs`, 34 in `flow/narrow.rs`, 23 in
`expression/check.rs`, plus two independent constant folders and one recognizer that matches
on **raw source text**. Three things were measured, not assumed:

1. **Wrapping an expression in parentheses — a semantic no-op in SurrealQL — turns off
   essentially all of the analyzer.** It destroys narrowing in four independent consumers,
   silences *every* expression-level diagnostic (E2004, W2015, E5001, E2008, E2030, E7005 …),
   disables field validation and result typing on a FROM target, and in two cases produces a
   hard error-severity **false positive** on valid input.
2. **Two recognizers for the same fact disagree about soundness.** `IF $n = NONE THEN THROW`
   on an `option<string | null>` narrows to `string` — dropping the `null`, which survives the
   guard at runtime. The row-side recognizer for the identical fact gets it right
   (`string | null`). That is not lost precision; it is a wrong type.
3. Recognizers that are byte-for-byte the same shape exist in four pairs, and one guard
   recognizer decides whether a function body is "guarded" by **grepping the body text for the
   substring `if`**.

This proposes one layer that answers "given this expression, in this environment, what do I
know?", built as a *normalizing predicate IR over canonical places, interpreted as a monotone
refinement of the `Kind` lattice*. Consumers stop matching AST shapes and start asking the
layer. **~55 shape-matching functions across six files collapse to three functions
(`place_of`, `eval`, `guard_of`) plus a data-only 11-variant `Atom` set and one
`refine`/`decide` pair** (itemized in §2.7).

---

## 1. Evidence: what is broken, observed

Everything in this section was run against a release build of `HEAD` of
`redesign-v3-foundation` (`CARGO_TARGET_DIR=/tmp/facts-target cargo build --release`) over a
scratch project at `/tmp/facts-probe` with the schema below and a host `.ts` probe file, via
`surrealguard generate`. Output is pasted verbatim from the generated registry.

```surql
DEFINE TABLE user SCHEMAFULL;
DEFINE FIELD name   ON user TYPE string;
DEFINE FIELD email  ON user TYPE option<string>;
DEFINE FIELD note   ON user TYPE option<string | null>;
DEFINE FIELD age    ON user TYPE option<int>;
DEFINE FIELD status ON user TYPE 'active' | 'inactive' | 'banned';
DEFINE FIELD tags   ON user TYPE option<array<string>>;
DEFINE INDEX user_email ON user FIELDS email UNIQUE;

DEFINE TABLE file SCHEMAFULL;
DEFINE FIELD title ON file TYPE string;
DEFINE FIELD owner ON file TYPE record<user | folder>;

DEFINE TABLE folder SCHEMAFULL;
DEFINE FIELD label ON folder TYPE string;

DEFINE TABLE sale SCHEMAFULL;
DEFINE FIELD price ON sale TYPE number;
DEFINE FIELD qty   ON sale TYPE int;
```

### 1.1 The headline: parentheses are a semantic no-op that the analyzer treats as a wall

`(expr)` lowers to `ast::Expr::Subquery(Statement::Expr(..))`
(`crates/syntax/src/lower/expr.rs:109`, `:359`). `const_eval` deliberately folds through it
(`analyzer/const_eval.rs:52-55`, with a comment saying so). **Nothing else does.** Four
independent consumers break:

| # | Query | Observed today | Correct |
|---|---|---|---|
| P1 | `SELECT name, email FROM user WHERE email != NONE` | `Array<{ email: string; name: string }>` | ✓ |
| P1' | `SELECT name, email FROM user WHERE (email != NONE)` | `Array<{ email?: string; name: string }>` | should match P1 |
| P2 | `LET $x = (SELECT name FROM ONLY user LIMIT 1); IF $x = NONE THEN THROW 'e' END; RETURN $x.name;` | `[null, undefined, string]` | ✓ |
| P2' | `… IF ($x = NONE) THEN THROW 'e' END; RETURN $x.name;` | `[null, undefined, unknown]` | should match P2 |
| P3 | `SELECT math::sum(price) * 2 AS t FROM sale GROUP ALL` | clean | ✓ |
| P3' | `SELECT (math::sum(price)) * 2 AS t FROM sale GROUP ALL` | **`error[E5002]: argument 1 to math::sum is a number, but an array is required`** | clean |
| P4 | `SELECT name FROM ONLY user WHERE email = 'a@b.c'` (UNIQUE index on `email`) | clean | ✓ |
| P4' | `SELECT name FROM ONLY user WHERE (email = 'a@b.c')` | **`warning[W4026]: this filter isn't provably single-row`** | clean |
| P5 | `SELECT nosuchfield FROM user` | `error[E1002]: user has no field nosuchfield` | ✓ |
| P5' | `SELECT nosuchfield FROM (user)` | **nothing; result `unknown`** | should match P5 |

**And it silences every expression-level diagnostic**, because `check_value_expression`
(`expression/check.rs:19-98`) has a `_ => {}` catch-all at `:97` that covers `Expr::Subquery`,
while the inference route for a subquery (`infer.rs:56-62` → `statement_value_kind` →
`Statement::Expr(e) => infer_expression_fact(e, ctx)`) never calls the checker at all:

| # | Query | Observed today |
|---|---|---|
| P6 | `SELECT VALUE email + 1 FROM user` | `error[E2004]: '+' can't combine a 'option<string>' and a 'int'` **plus** `warning[W2015]` |
| P6' | `SELECT VALUE (email + 1) FROM user` | **nothing**; `Array<unknown>` |
| P7 | `SELECT VALUE name.bogusmethod() FROM user` | `error[E5001]: 'string' has no method 'bogusmethod'` |
| P7' | `SELECT VALUE (name.bogusmethod()) FROM user` | **nothing**; `Array<unknown>` |

The rules that a pair of parentheses disables include 1027, 2004, 2005, 2008, 2015, 2030,
2031, 2032, 2036, 5001, 7003, 7005 and 7006. The same catch-all also skips `Expr::Closure`
entirely — **no closure body is ever checked** by `check_value_expression`.

P3' and P4' are the worst in the other direction: **error- and warning-severity false
positives on valid SurrealQL**, and P3' aborts `generate` for the entire workspace (the
FP-wave milestone in `docs/plans/2026-07-25-analyzer-gap-backlog.md` exists precisely to
eliminate this class).

The root causes are seven separate functions that each pattern-match `ast::Expr::Binary` /
`ast::Expr::Table` / a fixed variant list and fall off a `_ => …` cliff:
`narrow::where_effects` (`flow/narrow.rs:141`), `narrow::effects` (`:345`),
`infer::none_guarded_path` (`expression/infer.rs:772`),
`check::check_value_expression` (`expression/check.rs:97`),
`select::contains_column_aggregate` (`data/select.rs:1798`) with
`models_aggregate_shape` (`:1778`), `select::collect_equality_pinned_fields` (`:645`), and
`select::resolve_from_table` (`:127`).

Grammar/lowering provenance:
`SubQuery: ($) => seq('(', $._expression, ')')`
(`crates/syntax/tree-sitter-surrealql/grammar.js:1831`) →
`Expr::Subquery(Box<Statement::Expr(..)>)` (`crates/syntax/src/lower/expr.rs:359-364`).

This one gap propagates all the way into schema-level inference. With

```surql
DEFINE FUNCTION fn::pick ($u: option<{ name: string }>) { IF  $u = NONE  THEN THROW 'e' END; RETURN $u.name; };
DEFINE FUNCTION fn::pick2($u: option<{ name: string }>) { IF ($u = NONE) THEN THROW 'e' END; RETURN $u.name; };
```

```
"RETURN fn::pick({ name: 'a' });":  { result: [string]  }
"RETURN fn::pick2({ name: 'a' });": { result: [unknown] }
```

### 1.2 The narrowing recognizer inventory

Every recognizer in `crates/workspace/src/analyzer/flow/narrow.rs`, what it accepts, and the
equivalent SurrealQL it rejects. Line numbers are `flow/narrow.rs` unless noted.

| # | Recognizer | Accepts exactly | Rejects (equivalent meaning) |
|---|---|---|---|
| R1 | `effects` `:345` | `Expr::Binary` with `And`/`Or`/leaf, or a bare `Expr::Call` | `(cond)` (→ `Subquery`); `Expr::Block` guard `IF { c } {…}`; `Expr::Prefix{Not}` — **`!$x` and `!(…)` narrow nothing**; `Expr::Idiom` used as a truthiness guard (`IF $x THEN`); an `IF` used as an expression guard |
| R2 | `where_effects` `:141` | `Expr::Binary` with `And` (union) / `Or` (nothing) / leaf | same as R1, plus: takes **no `StatementEnv`** at all, so no `LET`-bound anything can be resolved; no `Prefix{Not}` arm; no bare-call arm — `WHERE type::is_string(email)` narrows nothing |
| R3 | `none_guard_path` `:544` | `X = NONE`/`X != NONE`/`X IS NONE`/`X IS NOT NONE`, either operand order, where `X` is `Expr::Param` or a `Start(Param) + Field*` idiom | `$x IS NOT NULL`-only paths on the flow side; `NOT ($x = NONE)`; `$x.tags[0] != NONE` (any non-`Field` part); a bare row field on the flow side (`email != NONE` in an `IF` inside a `DEFINE FIELD` body) |
| R4 | `row_none_null_effect` `:174` | `f != NONE`/`IS NOT NONE` → `StripNone`; `f != NULL`/`IS NOT NULL` → `NotNull`, `f` a **plain-`Field`-only** idiom | `f = NONE` (deliberate); `$param.f != NONE` (that is R3's domain, and the two do not share a place notion); `f[0] != NONE`; `f.g.*.h != NONE` |
| R5 | `row_literal_eq_effect` `:199` | `f = <scalar literal>`, either order | `f = <int literal>` where `f: option<int>` works, but **`status = 'active'` on `TYPE 'active' \| 'inactive' \| 'banned'` narrows nothing** (see §1.3); `f IN ['a','b']`; `f = $param` even when `$param`'s value is const-known; datetime/uuid/duration literals (excluded by `eq_literal_kind` `:317`) |
| R6 | `row_table_effect` `:222` | `type::table(f) = 'lit'` / `!= 'lit'`, either order, `f` plain-`Field` | **`record::tb(f) = 'lit'`; `meta::tb(f) = 'lit'`** — both are the same function (see §1.4); `type::table($p.f)`; `f.id().tb()` |
| R7 | `row_order_effect` `:243` | `f > lit` / `f >= lit` (and the flipped `lit < f`) → `NotNone` | `f > $param`; `f > 0 + 1`; `f BETWEEN`-style chains; `f > lit AND f < lit2` narrows via the AND but each leaf is re-matched from scratch |
| R8 | `leaf_effect` `:406` | R3 + `table_discriminant` | as R3/R6 |
| R9 | `in_effect` `:459` | `subject IN <collection>` where `op` is `BinaryOp::Other("IN")` (case-insensitive) and the collection is **a bare `Expr::Param` bound to `array<E>`/`set<E>`** | **`CONTAINS` (collection on the left)**; **`INSIDE`** (an explicit alias of `IN`); `subject IN ['a','b']` (an array *literal* — not a param); `subject IN $obj.field`; `NOT IN` (yields nothing, correctly, but is not even recognized as the negation of `IN`) |
| R10 | `is_record_effect` `:392` | bare `type::is_record($x)` / `type::is_record($x,'tbl')` used as a boolean, positive polarity only | the **rest of the `type::is_*` family**: `type::is_none`, `type::is_string`, `type::is_int`, `type::is_object`, `type::is_array`, `type::is_number`, `type::is_bool`, `type::is_datetime`, `type::is_uuid`, `type::is_decimal`, `type::is_duration`, `type::is_geometry`, `type::is_collection` — none narrow anything |
| R11 | `guard_verdict` `:876` | const fold, then `Expr::Binary` (NONE/NULL eq, `type::table` discriminant) or bare `type::is_record` call | compound `AND`/`OR` the const path could not settle — explicitly not decomposed (`:894-897`); parenthesized guards; `!`-negated guards |
| R12 | `collection_element_kind` `:494` | `Expr::Param` only | any expression that denotes a collection: `$x.tags`, `(SELECT VALUE t FROM …)`, `['a','b']`, `array::distinct($x)` |
| R13 | `guard_path_of` `:559` / `idiom_guard_path` `:569` | `Expr::Param`, or `Start(Param)` + one-or-more `IdiomPart::Field` | `$x.tags[0]`, `$x.*`, `$x.f.g()`, `$x?.f` (`IdiomPart::Optional`), `$x.{a}` (`Destructure`) — every non-`Field` part is an all-or-nothing cliff |

Plus two *value-side* helpers with the same cliff: `narrow_kind` `:717` and `eq_narrow` `:765`
(see §1.3), and the record-union transforms `narrow_record_to` `:779` /
`narrow_record_without` `:802`.

**Consumer sites reading these (verified by grep across `crates/`, excluding tests):**

| # | Consumer | Site |
|---|---|---|
| C1 | dead-branch verdicts | `flow/if_else.rs:42` → `branch_reachability_in_env` |
| C2 | THEN-branch narrowing | `flow/if_else.rs:87,95` |
| C3 | ELSE-branch narrowing | `flow/if_else.rs:136,143` |
| C4 | fall-through after a diverging guard, in a block | `flow/block.rs:199,208` |
| C5 | fall-through, at a source's top level | `pipeline.rs:417` |
| C6 | `A AND B` short-circuit into `B` (inference) | `expression/infer.rs:734,742` |
| C7 | `A AND B` short-circuit into `B` (checking) | `expression/check.rs:31,43` |
| C8 | SELECT `WHERE` row narrowing | `data/select.rs:2191` |
| C9 | const-only branch reachability | `const_eval.rs:268`, used at `flow/block.rs:253` |
| C10 | positional narrowing for the editor | `statement_env.rs:91` → `analysis.rs:115` → `query.rs:247,669` |

Four more consumers *should* be on this list and are not — they re-derive facts from the AST
themselves: aggregate promotion (`select.rs:1657-1845`), `ONLY` single-row cardinality
(`select.rs:614-707`), `FOR` element kind (`for_loop.rs:29-36`), and the second constant
folder in `schema/define/field.rs:416-596` (`ConstVal`, `fold_const`, `const_eq` — a
near-duplicate of `const_eval.rs`'s `ConstValue`, `const_eval`, `const_eq`).

### 1.3 The value side is broken too: refinement is an equality test, not a lattice meet

`eq_narrow` (`:765`) computes the literal's base kind and the field's base kind and requires
them to be `==`. For a field declared `TYPE 'active' | 'inactive' | 'banned'`, the kind is
`Either([Literal(String("active")), …])`; `literal_base_kind` (`kinds.rs:199`) returns `None`
for an `Either`, so `field_base` falls back to the whole `Either`, which is not `== Kind::String`,
and the refinement is dropped.

```
"SELECT name FROM user WHERE name = 'bob'":        { result: [Array<{ name: "bob" }>] }
"SELECT status FROM user WHERE status = 'active'": { result: [Array<{ status: "active" | "inactive" | "banned" }>] }
```

The same recognizer, the same predicate, the same intent — one field kind narrows and the
other does not. This is not a spelling problem; it is the absence of a *meet* operation. The
correct answer is `meet(Either([...]), Literal("active")) = Literal("active")`, and it needs
no special case.

### 1.4 Equivalent spellings that lose narrowing (observed)

```
"… IF type::table($f.owner) = 'user' THEN RETURN $f.owner END; …": [.., RecordId<"user"> | undefined, ..]
"… IF record::tb($f.owner)  = 'user' THEN RETURN $f.owner END; …": [.., RecordId<"user"> | RecordId<"folder"> | undefined, ..]
"… IF meta::tb($f.owner)    = 'user' THEN RETURN $f.owner END; …": [.., RecordId<"user"> | RecordId<"folder"> | undefined, ..]
```

```
"… IF !$x THEN THROW 'x' END; RETURN $x.name;":                [.., unknown]
"… IF type::is_none($x) THEN THROW 'x' END; RETURN $x.name;":  [.., unknown]
"… IF $x = NONE THEN THROW 'x' END; RETURN $x.name;":          [.., string]     -- the one that works
```

```
"SELECT name, email FROM user WHERE email != NONE":           Array<{ email: string;  name: string }>
"SELECT name, email FROM user WHERE email":                   Array<{ email?: string; name: string }>
"SELECT name, email FROM user WHERE !(email = NONE)":         Array<{ email?: string; name: string }>
"SELECT name, email FROM user WHERE type::is_string(email)":  Array<{ email?: string; name: string }>
```

```
"LET $rows = (SELECT name, email FROM user WHERE email != NONE); RETURN $rows.map(|$r| $r.email);":
  [null, Array<string>]
"LET $rows = (SELECT name, email FROM user); RETURN $rows.filter(|$r| $r.email != NONE).map(|$r| $r.email);":
  [null, Array<undefined | string>]
```

The last pair is instructive: a `.filter()` predicate is a positive guard over the surviving
elements, exactly as a `WHERE` is over the surviving rows. It is a *fifth* consumer of
`where_effects` that does not exist yet, and it cannot be added cheaply today because
`where_effects` keys refinements by schema-field path (`Vec<String>`) rather than by a place
that could also denote "the closure parameter's field".

Aliasing is not tracked at all:

```
"LET $x = (SELECT name FROM ONLY user LIMIT 1); IF $x = NONE THEN THROW 'e' END; LET $y = $x; RETURN $y.name;": [.., string]
"LET $x = (SELECT name FROM ONLY user LIMIT 1); LET $y = $x; IF $y = NONE THEN THROW 'e' END; RETURN $x.name;": [.., unknown]
```

### 1.5 Duplicated recognizers that disagree — including one that is unsound

This is the argument that matters most, because it is not about precision. When the same fact
is recognized in two places, the two implementations drift, and one of them ends up wrong.

**The NONE/NULL divergence, observed.** The field is
`DEFINE FIELD note ON user TYPE option<string | null>` — i.e. `Either([None, Null, String])`.

```
"SELECT note FROM user WHERE note != NONE":
  Array<{ note: string | null }>                              -- SOUND

"LET $n = (SELECT VALUE note FROM ONLY user LIMIT 1); IF $n = NONE THEN THROW 'e' END; RETURN $n;":
  [null, undefined, string]                                   -- UNSOUND: the `null` is gone
```

`NULL = NONE` is FALSE in SurrealDB, so a `NULL` value passes the guard and reaches the
`RETURN`. The generated TypeScript says `string`; the database can return `null`.

Root cause: the row path uses `Narrowing::StripNone` → `strip_variant` (`narrow.rs:749`),
which removes **only** `Kind::None`. The param path uses `Narrowing::NotNone` →
`narrow_out_none` (`infer.rs:865-879`), which removes **both** `Kind::None` **and**
`Kind::Null`. `narrow.rs:36-43` documents the distinction in a comment, correctly, and then
`narrow_kind`'s `NotNone` arm (`:720-723`) delegates straight back to the both-sentinel
version. Two recognizers for one fact; one of them is wrong.

Under the proposed design this cannot happen: `Atom::IsNotNone(p).refine(k)` is
`subtract(k, Kind::None)`, once, and it is the same function whether `p` is a param or a row
field. §7.3's property test would have caught it.

**The other duplicated pairs**, all confirmed by reading both sides:

| Pair | Sites | How they differ |
|---|---|---|
| "is this a `$param.field.field` path?" | `infer.rs:453 simple_idiom_path_key` vs `narrow.rs:569 idiom_guard_path` | identical shape, different return type (`String` vs `GuardPath`) |
| "is this the NONE/NULL literal?" | `infer.rs:798`, `narrow.rs:545`, `narrow.rs:306 none_or_null_literal`, `check.rs:645 sentinel_literal` | **four** recognizers, **three** different NULL policies |
| "is this `type::table(x)`?" | `narrow.rs:292 row_type_table_field` vs `narrow.rs:619 type_table_path` | same call shape, one keyed on row fields, one on params |
| constant folding | `const_eval.rs:22-45`, `schema/define/field.rs:416-455`, `select.rs:2329 literal_limit`, `pipeline.rs:842` | **four** folders, four different literal coverages |
| `AND`-narrowing vs `OR`-narrowing | `infer.rs:733` routes `AND` through the *general* engine (`effects`); `infer.rs:745` routes `OR` through the *legacy* `none_guarded_path` | the `OR` half of the language never reaches the general engine, so `$x IS NONE OR f($x)`, `($x = NONE) OR f($x)`, `$x = NONE OR $y = NONE OR f($x)` all narrow nothing |

**And the extreme case.** `pipeline.rs:957-968 fn_body_branches` — the carve-out that decides
whether a function body is "guarded" for diagnostic 5009 — is not an AST match at all. It is
`has_keyword(lower, "if") || has_keyword(lower, "for")` over the **raw lowercased body text**,
with no string- or comment-awareness. `DEFINE FUNCTION fn::x() { RETURN 'if you see this'; }`
is classified as guarded. `docs/plans/2026-07-25-analyzer-gap-backlog.md:645` records that
this carve-out is *load-bearing* for the workshop oracle.

### 1.6 Narrowing that is computed but not delivered

Three places compute a correct narrowing and then fail to hand it to a consumer that needs it:

1. **Positional narrowing is missing for in-expression guards.** `record_narrowing`
   (`narrow.rs:692`, `:708`) fires only when `apply_effects_over` is called with
   `region: Some(_)`. The `AND` short-circuit sites (`infer.rs:742`, `check.rs:43`) pass
   `None`, and `with_guard_narrowed` (`infer.rs:816-839`) bypasses `apply_effects_over`
   entirely by calling `ctx.define_narrowed_path` directly. So `= NONE OR` / `!= NONE AND`
   narrowing is **never** visible to hover, by construction.
2. **A narrowed field path does not benefit its sub-paths.** `step_idiom_kind`
   (`infer.rs:492-496`) looks up `env.narrowed_path` under the **exact** key. After
   `IF $file.folder != NONE`, the key `"file.folder"` is narrowed, but reading
   `$file.folder.name` builds the key `"file.folder.name"`, misses, and falls back to walking
   from `$file` — where `field_of_kind` (`infer.rs:548-565`) rejects `Kind::Either` and
   returns `None`. The narrowing is present in the env and unreachable from the read.
3. **Checking never consults narrowing at all.** `idiom_prefix_kinds` (`infer.rs:335-356`)
   and every contract in `check.rs:300-340` read the declared kind. So
   `IF $x.y != NONE { RETURN $x.y.len(); }` still reports
   **`error[E5001]: 'option<string>' has no method 'len'`** — a false positive on code the
   analyzer has already proven safe.

A single `KindOracle` (§2.6) consulted by both inference and checking is the direct fix for
all three.

### 1.7 The satellite inventory

Cross-checked by a full read of every file listed. Distinct AST-shape match sites:

| File | Sites |
|---|---|
| `data/select.rs` | 60 |
| `expression/infer.rs` | 34 |
| `flow/narrow.rs` | 34 |
| `expression/check.rs` | 23 |
| `analyzer/pipeline.rs` | 11 (one of them text-based) |
| `const_eval.rs` | 9 |
| `flow/if_else.rs` | 5 |
| `flow/block.rs` | 5 |
| `flow/let_stmt.rs` | 4 (two of them name lists) |
| `flow/for_loop.rs` | 3 |
| `analyzer/statement.rs` | 1 (exhaustive — a new variant is a compile error, as it should be) |
| **total (audited)** | **≈189** |

(`data/mutation.rs` adds ~26 more, un-audited here; 243 `ast::Expr::` / `ast::IdiomPart::`
references exist across `analyzer/` in total.) The recurring cliffs, in descending order of
blast radius:

1. **`plain_field_segments` (`expression/infer.rs:317`) is the de-facto "is this a field
   path?" oracle**, consulted from **17 call sites in `select.rs` alone** (`:54` OMIT, `:65`
   GROUP keys, `:335` FETCH, `:368` SPLIT, `:433` ORDER BY, `:660`, `:719`, `:952`, `:1259`,
   `:1455`, `:1669`, `:1953`, `:1995`, `:2077`, `:2123`, `:2139`, `:2153`) plus `narrow.rs:288`
   and `schema/define/field.rs:306`. It returns `Some` only when *every* `IdiomPart` is
   `Field`. One `Index`, `All`, `Where`, `Method`, `Optional`, `Destructure`, `Recurse` or
   `Start` part makes the whole idiom invisible to all of them at once. `OMIT address['city']`,
   `SPLIT tags[*]`, `ORDER BY meta.a[0]`, `FETCH team?.owner` are all silently unhandled and
   silently unchecked.
2. **Parenthesization** (§1.1) — defeats 6 distinct recognizers; handled in exactly one place.
3. **Call-path matching is exact-string and case-sensitive** in at least 7 places
   (`select.rs:1000` `count`, `:1844` the 18-name aggregate list, `:435` `rand`, `:1532`
   `type::field`, `:1565`; `narrow.rs:298`, `:394`, `:622`). `normalize_function_path`
   (`syntax/src/lower/expr.rs:639`) is `trim().replace("::is::","::is_")` and does **no case
   folding**, so `MATH::SUM(x)` under `GROUP ALL` is not an aggregate.
4. **Operand order is handled ad hoc.** `collect_equality_pinned_fields` (`select.rs:650`),
   `row_*_effect`, `none_guard_path`, `table_discriminant` each re-implement "try both sides".
   `row_order_effect` additionally re-implements operator flipping (`flip_order`, `:271`).
5. **Two independent constant folders** — `const_eval.rs:22-45` and
   `schema/define/field.rs:416-455` — with different literal coverage and different soundness
   edges. `select.rs` calls **neither**: `literal_limit` (`:2329`) is a third, weaker fold
   that accepts only a bare `Literal::Int`, so `LIMIT (1)` and `LIMIT 1 + 0` lose both the
   `ONLY` single-row proof and the array's max-length.
6. **`Kind::Either` is not distributed** in the flat `matches!(k, Kind::Array(..) | Kind::Set(..))`
   tests at `for_loop.rs:29-36` (already partly fixed), `check.rs:305-315`, `select.rs:1688`,
   `query.rs:1667` (which uses `find_map` — first arm wins — and is wrong for
   `array<int> | array<string>`). More seriously, `field_of_kind` (`infer.rs:548-565`) —
   the primitive under `step_field_path`, which *every* field-path narrowing goes through —
   accepts only `Kind::Literal(Object)`, single-target `Kind::Record`, and `Kind::Array`.
   It rejects `Kind::Either` outright, so **any optional intermediate segment kills the
   whole path**, and rejects multi-target `Kind::Record([a, b])`, `Kind::Set`, bare
   `Kind::Object` and `Kind::Any`.
7. **Special-cased parameter names are maintained as five overlapping hand-written lists**
   that do not agree: `PROTECTED` (`let_stmt.rs:14-17`), `CONTEXT_ONLY_PARAMS`
   (`expression/mod.rs:85`), `session_context_params()` (`context_params.rs:214-220`),
   `is_reserved_session_param` (`context_params.rs:42`), and the field/event/permission
   binding maps (`context_params.rs:164-198`, `schema/define/permissions.rs:87-103`).
   Consequences: `$this` is protected but not context-only, so a top-level `RETURN $this.x`
   is silently recorded as a **required host parameter** while `$value` correctly emits 6005;
   **`$parent` is protected and bound by nothing**, so a correlated subquery
   `… WHERE id IN (SELECT p FROM b WHERE q = $parent.id)` demands `$parent` from the caller
   and codegen emits it; `$self` is bound in three contexts but absent from `PROTECTED`;
   `$scope` is seeded but shadowable with no 6007. `PlaceRoot::Param` plus one authoritative
   binding table replaces all five.
8. **Divergence is under-modelled at two sequence levels.** `block_diverges`
   (`block.rs:267-272`) inspects **only the last statement**. `statement_diverges`
   (`:215-235`) does not recognize a `Statement::Expr` whose expression is a diverging block
   or IF-as-value. And the top-level statement loop (`pipeline.rs:398-468`) has **no `Flow`
   at all** — top-level `RETURN`s are not collected into an exit-set type, there is no 4006
   for statements after a top-level `THROW`, and `statement_diverges` is never consulted.
   Only `apply_fall_through_narrowing` (`:417`) was retrofitted, which is why commit
   `6fce137` fixed narrowing there and nothing else.

---

## 2. The abstraction

### 2.1 What is actually being confused

Every recognizer above conflates three separable questions:

1. **Denotation** — *which value does this expression name?* `(x)`, `x`, and `$x` after
   `LET $x = …` all denote the same thing; `type::table(f)`, `record::tb(f)` and `meta::tb(f)`
   all denote the same thing.
2. **Assertion** — *what does this boolean expression claim?* `a != NONE`,
   `NOT (a = NONE)`, `!(a IS NONE)`, and `type::is_string(a)` (partially) all claim
   overlapping things about the same place.
3. **Refinement** — *what does that claim do to a `Kind`?* This is a lattice operation
   (meet with the claim's characteristic kind), not a table of ad-hoc transforms.

Separating them is the whole design. (1) is a *normalization*; (2) is a *small closed IR*;
(3) is a *lattice*. Each is independently testable and independently sound.

### 2.2 Layer 1 — `Place`: the canonical denotation of a narrowable location

```rust
/// A location whose kind can be refined. Canonical: two syntactically different
/// expressions that provably denote the same location produce the same `Place`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Place {
    pub root: PlaceRoot,
    /// Only *naming* steps. A step that can fail, filter, or compute is not a
    /// place — see `Step` and §5.
    pub path: Vec<Step>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PlaceRoot {
    /// `$x` — a `LET` binding, a function parameter, a `FOR` variable, a closure
    /// parameter, or a seeded session param (`$auth`, `$session`, `$this`,
    /// `$value`, `$before`, `$after`, `$input`, `$parent`, `$event`).
    Param(String),
    /// A bare field of the row currently in scope: a SELECT's projected row, a
    /// `DEFINE FIELD` clause's `$this`, an `array::filter` closure's element.
    /// The *identity of the row* is carried by the environment, not by the
    /// place, exactly as `row_field_path` does today.
    RowField,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Step {
    /// `.name`
    Field(String),
    /// `[0]`, `[-1]` — a constant index. Narrowing an element is sound only for
    /// a fixed-length tuple kind; see §5.
    Index(i64),
}
```

`Place` replaces `GuardPath` (`narrow.rs:63`) *and* the `Vec<String>` field path in
`RowEffect` (`:124`) *and* the `plain_field_segments` result, unifying three notions that are
the same thing. `Place::key()` (the `param.field.field` string) keeps the existing
`narrowed_path` / `NarrowingAnalysis::path` wire format unchanged — this matters for §2.7.

Normalization to a `Place` is one function and is the *only* place that sees `ast::Expr`:

```rust
/// The place an expression denotes, or `None` when it denotes no fixed location.
///
/// Sees through: parenthesization (`Expr::Subquery(Statement::Expr(..))`),
/// single-expression blocks, no-op casts, and `LET`-alias chains resolvable in
/// `env`. Refuses: any step that can compute, filter, or fail.
pub fn place_of(expr: &ast::Expr, env: &StatementEnv) -> Option<Place>;
```

### 2.3 Layer 1b — `Term`: the canonical denotation of a *value*

```rust
/// What an expression denotes, as far as the layer can prove.
#[derive(Clone, Debug, PartialEq)]
pub enum Term {
    /// A refinable location.
    Place(Place),
    /// A statically known value. Subsumes both existing constant folders.
    Const(ConstValue),
    /// A known-kind value with no known identity: a subquery, a call, an
    /// arithmetic result. Carries the kind so `IN`/comparison atoms can use it.
    Opaque { kind: Option<Kind> },
    /// A collection whose element kind is known: an array literal, a
    /// `array<E>`-kinded place, a `(SELECT VALUE f FROM t)` subquery.
    Collection { element: Kind, len: Option<usize> },
    /// A projection *of* a place that discriminates it. This is how
    /// `type::table($x)` stops being a special case: it is a `Discriminant`
    /// term over the place `$x`.
    Discriminant { of: Place, kind: DiscriminantKind },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscriminantKind {
    /// `type::table(x)` ≡ `record::tb(x)` ≡ `meta::tb(x)` ≡ `x.tb()` — the
    /// table name of a record link.
    RecordTable,
    /// `type::type(x)` — the runtime type name.
    RuntimeType,
}
```

`Term::Discriminant` is the generalization that kills R6: the three spellings normalize to
one term, and a fourth spelling costs one line in the normalizer rather than a new recognizer
in every consumer.

`Term::Const` unifies `const_eval::ConstValue` and `schema/define/field.rs::ConstVal`. The
folder becomes `fn eval(expr, env) -> Term`, which subsumes both, sees through parentheses
(as `const_eval` already does) *and* through `LET` bindings and `DEFINE PARAM` defaults
(which neither does today).

### 2.4 Layer 2 — `Guard`: the predicate IR

```rust
/// What a boolean expression asserts. Closed, small, and negation-normal:
/// `Not` is pushed into atoms during construction, so no consumer ever sees it.
#[derive(Clone, Debug, PartialEq)]
pub enum Guard {
    /// Every conjunct holds.
    All(Vec<Guard>),
    /// At least one disjunct holds.
    Any(Vec<Guard>),
    Atom(Atom),
    /// Provably true / provably false — the constant folder's result, lifted
    /// into the same IR so `guard_verdict`'s "const path first" special case
    /// disappears.
    True,
    False,
    /// Nothing is known. Distinct from `True`/`False`: an `All` containing
    /// `Unknown` still contributes its other conjuncts; an `Any` containing
    /// `Unknown` contributes nothing.
    Unknown,
}

/// One indivisible claim. `polarity` is folded in at construction: `IsNone` and
/// `IsNotNone` are separate atoms rather than one atom plus a sign, so
/// `refine` is total and no consumer can forget to flip.
#[derive(Clone, Debug, PartialEq)]
pub enum Atom {
    /// `p = NONE`, `p IS NONE`, `type::is_none(p)`
    IsNone(Place),
    /// `p != NONE`, `p IS NOT NONE`, `NOT type::is_none(p)`
    IsNotNone(Place),
    /// `p = NULL` / `p != NULL` — separate from NONE, deliberately (see
    /// `narrow.rs:36-43`: a NULL survives `!= NONE` and vice versa).
    IsNull(Place),
    IsNotNull(Place),
    /// `p` used as a truthiness guard, `IF p THEN …`, `WHERE p`.
    /// Strictly stronger than `IsNotNone` + `IsNotNull` (also excludes
    /// `false`, `0`, `''`, `[]`), but conservatively refines to the same thing
    /// unless the kind is a literal union.
    Truthy(Place),
    /// `p = <term>` where the term is a constant. Refines by lattice meet with
    /// the constant's singleton kind.
    Eq(Place, ConstValue),
    /// `p != <const>`. Refines only a literal-union kind (by subtraction);
    /// otherwise a no-op.
    NotEq(Place, ConstValue),
    /// `p <op> <term>` for an ordering operator, normalized so the place is
    /// always on the left (`18 < age` becomes `age > 18`).
    Ord(Place, OrdOp, Term),
    /// The place's runtime kind is (not) `k`. This is where the whole
    /// `type::is_*` family lands, plus `Term::Discriminant{RuntimeType}`
    /// compared to a string.
    HasKind(Place, Kind),
    NotKind(Place, Kind),
    /// The place is a record whose table is (not) in this set. `type::table(p)
    /// = 'a'`, `record::tb(p) IN ['a','b']`, `type::is_record(p,'a')` all land
    /// here — one atom, many spellings.
    InTables(Place, BTreeSet<String>),
    NotInTables(Place, BTreeSet<String>),
    /// `p IN <coll>`, `<coll> CONTAINS p`, `p INSIDE <coll>` — one atom, three
    /// spellings, normalized so the member is always the place.
    Member(Place, Term),
}
```

The construction function is the second and last place that sees `ast::Expr`:

```rust
/// Lower a boolean expression to its `Guard`. `polarity` selects the region:
/// `true` for where the expression is truthy, `false` for where it is falsy.
/// De Morgan is applied here, once, rather than in each consumer.
pub fn guard_of(expr: &ast::Expr, polarity: bool, env: &StatementEnv) -> Guard;
```

`guard_of(e, false)` is `guard_of(e, true)` negated *by construction*, which is what makes
`positive_effects`/`negative_effects` (`:106`, `:112`), `flip_none` (`:506`),
`flip_table` (`:514`), `flip_order` (`:271`), and `Verdict::negate` (`:855`) collapse into
one recursive polarity parameter. It is also what makes `!(email = NONE)` and
`NOT type::is_none($x)` work with no new code.

### 2.5 Layer 3 — refinement as a lattice operation

```rust
/// The refinement an atom proves, as a function on kinds. Total, monotone,
/// and conservative: `refine` returns the *meet* of the input kind with the
/// atom's characteristic kind, and returns the input unchanged whenever the
/// meet cannot be computed. It never returns a kind that excludes a value the
/// input admits and the atom does not rule out.
impl Atom {
    pub fn refine(&self, kind: &Kind, schema: &SchemaIndex) -> Kind;

    /// Whether this atom is provably decided by `kind` alone.
    /// `AlwaysTrue` iff every value of `kind` satisfies it; `AlwaysFalse` iff
    /// no value of `kind` does; `Unknown` otherwise.
    pub fn decide(&self, kind: &Kind) -> Verdict;
}
```

The two are related by one invariant that must hold for every atom, and which is the
mechanical statement of *prove-or-stay-silent*:

> **Refinement soundness.** For every atom `a` and kind `k`,
> `values(a.refine(k)) ⊇ { v ∈ values(k) | a holds of v }`, and
> `a.refine(k) ⊑ k` (refinement only ever narrows).
>
> **Decision soundness.** `a.decide(k) = AlwaysFalse` implies
> `{ v ∈ values(k) | a holds of v } = ∅`; `AlwaysTrue` implies it is all of
> `values(k)`.

This is a property test, not a comment (§7.3): both directions are checkable by enumerating
a finite kind universe.

The workhorse underneath is a genuine lattice meet on `Kind`:

```rust
/// The greatest kind below both, or `None` when the meet is not representable
/// in `Kind` (in which case callers keep the wider input — never invent).
///
/// - `meet(Either([None, String]), String)          = String`
/// - `meet(Either([Lit"a", Lit"b", Lit"c"]), Lit"a") = Lit"a"`   ← fixes §1.3
/// - `meet(Record([user, folder]), Record([user]))  = Record([user])`
/// - `meet(Any, k)                                  = k`
/// - `meet(String, Int)                             = None (bottom; caller keeps input)`
pub fn meet(a: &Kind, b: &Kind) -> Option<Kind>;

/// Kind subtraction, for the `Not*` atoms: `k` minus everything in `b`.
/// - `subtract(Either([None, String]), None)        = String`     ← StripNone
/// - `subtract(Record([user, folder]), Record([folder])) = Record([user])`
/// - `subtract(String, Lit"a")                      = String` (not representable)
pub fn subtract(k: &Kind, b: &Kind) -> Kind;
```

`meet` and `subtract` replace `narrow_kind` (`:717`), `strip_variant` (`:749`), `eq_narrow`
(`:765`), `narrow_record_to` (`:779`), `narrow_record_without` (`:802`), `narrow_out_none`,
and the four `kind_can_be_*` / `kind_has_non_*` predicates (`:1033-1068`) — those become
`meet(k, Kind::None) != bottom` and friends.

### 2.6 The one API every consumer reads

```rust
/// The refinements a guard proves, as a map from place to refined kind, given
/// the kinds currently in force. This is the whole public surface.
pub struct Facts {
    refined: BTreeMap<Place, Kind>,
    verdict: Verdict,
}

impl Guard {
    /// Interpret this guard against an environment.
    ///
    /// `All` intersects (meets) per place across conjuncts.
    /// `Any` joins (unions) per place, and yields a refinement for a place
    ///   ONLY when EVERY disjunct refines that place — otherwise the place is
    ///   absent. This is the P2 `OR` rule from the narrowing-result-types
    ///   design, obtained for free rather than as a special case.
    /// `Unknown` contributes nothing.
    pub fn facts(&self, env: &dyn KindOracle, schema: &SchemaIndex) -> Facts;
}

/// How the interpreter looks up the kind currently in force for a place.
/// Implemented by `StatementEnv` (params, `LET`s, narrowed paths) and by the
/// SELECT row post-pass (projected object leaves) and by a closure's element
/// scope. THIS is what lets `where_effects` and `positive_effects` be the same
/// function — today they are two, because the row consumer has no env.
pub trait KindOracle {
    fn kind_of(&self, place: &Place) -> Option<Kind>;
    /// Whether this place was tightened by a prior *flow* narrowing, which is
    /// the gate `path_kind` (`narrow.rs:981`) uses to keep dead-branch folding
    /// off base bindings. Preserved verbatim.
    fn is_flow_narrowed(&self, place: &Place) -> bool { false }
}
```

`Facts` is deliberately *not* `Vec<Effect>`: a map keyed by `Place` means a place refined
twice in one guard (`age > 18 AND age != NONE`) is meet-combined once, rather than applied
twice in list order with the second silently no-oping.

### 2.7 How each consumer is re-expressed

| Consumer | Today | After |
|---|---|---|
| C1 dead branches (`if_else.rs:42`) | `branch_reachability_in_env` → `guard_verdict` → const path, then 3 hand recognizers | `guard_of(cond, true, env).facts(env, schema).verdict`. The const path is not a special case: a constant guard lowers to `Guard::True`/`False`. The "flow-narrowed subjects only" gate stays, as `KindOracle::is_flow_narrowed`. |
| C2/C3 branch bodies (`if_else.rs:87,136`) | `positive_effects` / `negative_effects` + `apply_effects_over` | `guard_of(cond, polarity, env).facts(..)`, then one `apply(ctx, facts, region)`. Same region, same `record_narrowing` call. |
| C4/C5 fall-through (`block.rs:199`, `pipeline.rs:417`) | union of `negative_effects` over branches | `Guard::All(branches.map(|b| guard_of(b.cond, false, env)))` — the conjunction is now explicit and the interpreter meets it. |
| C6/C7 `A AND B` (`infer.rs:734`, `check.rs:31`) | `positive_effects(lhs)` | `guard_of(lhs, true, env).facts(..)`. Short-circuit direction is preserved (still lhs→rhs only). |
| C8 SELECT `WHERE` (`select.rs:2191`) | separate `where_effects` + `RowEffect` + `narrow_kind_at_path` | the *same* `guard_of` + `facts`, with a `KindOracle` over the projected object literal and `PlaceRoot::RowField`. `where_effects`, `RowEffect`, `row_leaf_effect`, `row_none_null_effect`, `row_literal_eq_effect`, `row_table_effect`, `row_order_effect`, `row_field_path`, `row_type_table_field`, `flip_order`, `is_ordering_literal` — **11 functions, ~180 lines — delete entirely.** |
| C9 const reachability (`const_eval.rs:268`) | its own folder | `Guard::True`/`False` from `eval`. `const_eval.rs` keeps its public `const_eval`/`const_eval_bool` as thin wrappers during migration, then goes. |
| C10 positional narrowing (`query.rs:247,669`) | reads `AnalysisOutput.narrowings` | **unchanged.** `apply` still calls `ctx.record_narrowing(place.key(), region, kind)`; `Place::key()` produces the identical `param.field.field` string. The LSP is untouched and remains cache-only. |
| *new* aggregate promotion (`select.rs:1657-1845`) | `contains_column_aggregate` + `models_aggregate_shape` + `aggregate_operand_kind`, 3 parallel Expr walks | one walk driven by `Term` normalization: a projection is aggregate-bearing iff its `Term` contains an aggregate-classified call. Parenthesization and `Array`/`Object` wrapping stop mattering — fixes P3'. |
| *new* `ONLY` cardinality (`select.rs:614`) | `collect_equality_pinned_fields` + `is_row_independent` + `literal_limit` | `guard_of(where, true, env)`, then read `Atom::Eq(place, _)` and `Atom::InTables` out of the `All` conjuncts. `literal_limit` becomes `eval(limit)` — fixes P4' and `LIMIT (1)`. |
| *new* `.filter(\|$r\| …)` element narrowing | absent | the closure body is a guard; the closure param is a `PlaceRoot::Param`; the element kind is refined by `facts`. §1.4's last case starts working. |
| *new* `FOR $x IN e` element kind (`for_loop.rs:29`) | flat `matches!` | `Term::Collection { element }` — an `option<array<T>>` iterable normalizes to a collection term with `element = T` and a residual `IsNone` obligation. |
| *new* checking-side reads (`check.rs:300-340`, `infer.rs:335-356`) | read the declared kind; narrowing invisible | resolve through `KindOracle::kind_of(place)`, which consults `narrowed_path` by longest-prefix rather than exact key — fixes F31 and §1.6(2)(3) together |
| *new* legacy `OR` narrowing (`infer.rs:772 none_guarded_path`) | its own 40-line recognizer, `OR`-only | deleted; `guard_of(lhs, false, env)` on the `OR` short-circuit, symmetric with the `AND` case — fixes F32 |

**Recognizer collapse.** Counted by function:

| Group | Functions | Fate |
|---|---|---|
| narrowing recognizers R1–R13 | 13 | → `guard_of` + `Atom` |
| `where_effects` row family (`where_effects`, `row_leaf_effect`, `row_none_null_effect`, `row_literal_eq_effect`, `row_table_effect`, `row_order_effect`, `row_field_path`, `row_type_table_field`, `flip_order`, `is_ordering_literal`, `eq_literal_kind`) | 11 | deleted; same `guard_of` |
| value-side transforms (`narrow_kind`, `strip_variant`, `eq_narrow`, `narrow_record_to`, `narrow_record_without`, `narrow_out_none`, `kind_can_be_none`/`kind_has_non_none`/`kind_can_be_null`/`kind_has_non_null`) | 10 | → `meet` + `subtract` |
| verdict recognizers (`guard_verdict`, `binary_verdict`, `is_record_verdict`, `sentinel_eq_verdict`, `table_eq_verdict`, `record_tables`) | 6 | → `Atom::decide` |
| place/path recognizers (`guard_path_of`, `idiom_guard_path`, `simple_idiom_path_key`, `none_guard_path`, `none_or_null_guard`, `string_literal`, `type_table_path`, `type_table_arg`, `discriminated_path`, `table_discriminant`, `collection_element_kind`, `sentinel_literal`) | 12 | → `place_of` + `eval` |
| constant folders (`const_eval` family, `fold_const` family, `literal_limit`, `pipeline.rs:842`) | 4 groups | → `eval` |
| legacy `OR` narrowing (`none_guarded_path`, `with_guard_narrowed`, `with_none_narrowed`) | 3 | → `guard_of` (the general engine) |
| **total** | **~55 functions across 6 files** | **→ 3 functions + one 11-variant data enum + `meet`/`subtract` + `refine`/`decide`** |

`narrow.rs` goes from 2086 lines (of which ~1090 are production) to an estimated ~450 lines of
production code across `facts/{place,term,guard,refine}.rs`, and the four duplicated-recognizer
pairs of §1.5 stop existing as a category.

---

## 3. Why this abstraction and not the others

The four candidates, weighed against how this codebase is actually built.

### 3.1 Occurrence typing with a control-flow graph (TypeScript / Flow) — **rejected in part, adopted in part**

TypeScript's checker builds a real CFG and attaches a *flow node* to every identifier
reference; `getFlowTypeOfReference` walks backwards through the graph applying narrowing at
each condition node. It handles loops by fixpoint over back-edges and aliasing by
"discriminant property" caching.

What this codebase has instead: no CFG at all. `analyze_block_flow` (`flow/block.rs:90`) is a
straight recursive walk producing `Flow { returns, value, diverges }`; the environment is a
scope stack (`StatementEnv::fork_child_scope`, `statement_env.rs:59`); divergence is a
syntactic predicate (`statement_diverges`, `block.rs:214`). Building a CFG means rewriting
`block.rs`, `if_else.rs`, `for_loop.rs`, `statement.rs` and `pipeline.rs`, re-deriving the
emission point of every diagnostic that currently fires during the walk, and re-deriving the
scope-end ranges that `NarrowingAnalysis` depends on (`block.rs:206`). That is a rewrite of
the analyzer's spine to buy something SurrealQL does not need: **SurrealQL has no `goto`, no
labelled break out of arbitrary nesting, and no early-exit forms beyond
`RETURN`/`THROW`/`BREAK`/`CONTINUE`, all four of which `statement_diverges` already models.**
The AST walk *is* a structured, reducible CFG; the CFG would be isomorphic to it.

So: **adopt occurrence typing's fact model** — a normalized predicate, a polarity, a refine
function, per-occurrence results — and **reject its CFG machinery**. Concretely, §2.4's
`Guard` is TypeScript's narrowing-condition analysis with `Not` pushed to the leaves, and
§2.7's `region`-recorded `NarrowingAnalysis` is its per-reference flow type, obtained from
lexical scope rather than a graph.

The honest cost of declining the CFG is stated in §5: no join-point narrowing across
branches that both narrow the same place differently, and no loop fixpoint.

### 3.2 Datalog-style fact propagation — **rejected**

Attractive on paper: facts are exactly what is wanted, and `σ`-pushdown through a `WHERE` is
a textbook relational-algebra derivation.

Three concrete reasons it loses here:

1. **Subtyping is the hard part, and Datalog does not have it.** The real bug in §1.3 is a
   missing `meet` on `Kind`. A least-fixpoint over derived facts gives you *derivability*,
   not *soundness under a lattice order*; you would encode `Kind`'s subtype relation as EDB
   rules and re-derive it per query. That is strictly more machinery for the same answer that
   `meet(a,b)` gives directly, and it is much harder to prove the prove-or-stay-silent
   property about.
2. **The positional requirement is awkward.** `NarrowingAnalysis` (`analysis.rs:115`) needs
   `(path, byte-range, kind)`. A byte range is a *scope*, which a lexical walk produces
   naturally and a fact database only reproduces with explicit provenance on every derivation.
3. **Cost.** Full workspace analysis is ~420 ms wall for the 149-source workshop today
   (measured: `real 0.45 / 0.42 / 0.42`, three runs of the release binary over
   `/Users/drewridley/Documents/Projects/workshop/database`, including process start).
   Standing up a fact database per source, or one incrementally-maintained database for the
   workspace, is a new dependency and a new memory profile for a workload that currently
   allocates a `Vec<Effect>` per guard. §6.5 estimates the proposed design at +5–8%; a
   Datalog engine is not in that budget.

Datalog's *idea* survives: `Facts` is a fact set, and `Guard::All`/`Any` are conjunction and
disjunction. It is the *engine* that is rejected.

### 3.3 A predicate/constraint IR that recognizers lower into — **adopted, but insufficient alone**

This is §2.4, and it is half the answer. It fixes every spelling-equivalence bug in §1.1 and
§1.4. It does **not** fix §1.3 (`status = 'active'`), because that failure is on the *value*
side: the predicate was recognized correctly and the refinement was dropped. A constraint IR
without a lattice would keep that bug and would keep the seven ad-hoc kind transforms in
`narrow.rs:717-823`. Hence §2.5 is not optional.

### 3.4 Abstract interpretation over a lattice of `Kind` + refinement facts — **recommended**

This is the union of §2.2–§2.6, and it is what is being proposed, with one honest
qualification: **it is abstract interpretation in the sense of "a sound monotone
interpretation of the program over an abstract domain", not in the sense of "iterate a
transfer function to a fixpoint".** There is no widening operator and no loop iteration
(§5.4). The domain is `Kind` ordered by subtyping, joined by `Kind::either` and met by
§2.5's `meet`; the transfer function is `Guard::facts`; the interpretation is the existing
lexical walk.

The reason this is the right shape for *this* codebase specifically: the analyzer already
carries `ExpressionFact` (`crates/workspace/src/expression.rs:25`) with `kind`, `value`,
`value_class`, `partial` and `dependencies` — an abstract value in everything but name, with
`PartialReason` already serving as the "cannot prove" channel. The proposal is not a new
paradigm; it is finishing the one the codebase already chose, and connecting it to the guard
side, which was built separately and never got it.

### 3.5 The strongest argument against — stated, not hidden

**This layer centralizes exactly what `MEMORY.md` says not to centralize.**
`per_statement_analyzers_own_logic` says: *don't centralize multiple statement kinds behind
one shared entry point, even when identical today.* Two rebuttals and one concession:

- *Rebuttal 1 — different axis.* That rule is about **statement semantics**: a `CREATE`'s
  write contract genuinely differs from an `UPDATE`'s, and merging them predicts a
  convergence that will not happen. This layer is about **expression denotation**, where the
  convergence is not predicted but *definitional*: `(x)` means `x` in a `WHERE`, in an `IF`,
  in a `LIMIT` and in an aggregate, and no future SurrealQL revision will change that. The
  evidence in §1.1 is that treating them as separate has already cost four bugs.
- *Rebuttal 2 — the layer has no policy.* `Guard`/`Atom`/`Facts` emit no diagnostics and know
  no severities. Every consumer keeps its own policy: which polarity it applies, whether it
  may act on `Verdict::AlwaysFalse`, which region it records. `contract_first_diagnostics`
  is untouched.
- *Concession — the blast radius genuinely grows, and this is the real cost.* Today a wrong
  refinement in `row_order_effect` can only produce a wrong SELECT row type; it structurally
  cannot reach `guard_verdict` and grey a live branch. After this, one wrong `Atom::refine`
  is simultaneously a wrong result type, a wrong hover, a wrongly-greyed branch, and possibly
  a false E5002. §7.1 names this the single biggest risk in the proposal, and §7.3 is the
  mitigation (per-atom property tests of the two soundness invariants, plus consumer
  capability gating in Stage 4 so that, e.g., dead-branch folding only consults atoms
  explicitly marked decidable).

  The counter-evidence is worth weighing against the concession, though: **§1.5's F30 is an
  unsound generated type that exists precisely *because* the two recognizers are separate.**
  Separation did not contain the blast radius; it only meant nobody audited it. The choice is
  not "small blast radius vs large" but "many small unaudited surfaces vs one audited one".

---

## 4. Cases this fixes

Every "today" column below is observed output from the release binary, not predicted.

| # | Query | Today (observed) | After |
|---|---|---|---|
| F1 | `SELECT name, email FROM user WHERE (email != NONE)` | `Array<{ email?: string; name: string }>` | `Array<{ email: string; name: string }>` |
| F2 | `LET $x = (SELECT name FROM ONLY user LIMIT 1); IF ($x = NONE) THEN THROW 'e' END; RETURN $x.name;` | `[null, undefined, unknown]` | `[null, undefined, string]` |
| F3 | `SELECT (math::sum(price)) * 2 AS t FROM sale GROUP ALL` | **`error[E5002]`** (aborts `generate`) | clean, `Array<{ t: number }>` |
| F4 | `SELECT name FROM ONLY user WHERE (email = 'a@b.c')` (UNIQUE `email`) | **`warning[W4026]`** | clean |
| F5 | `SELECT nosuchfield FROM (user)` | no diagnostic, `unknown` | `error[E1002]`, and `SELECT name FROM (user)` becomes `Array<{ name: string }>` |
| F6 | `RETURN fn::pick2(…)` where the body guards with `IF ($u = NONE)` | `unknown` | `string` |
| F7 | `… IF record::tb($f.owner) = 'user' THEN RETURN $f.owner END; …` | `RecordId<"user"> \| RecordId<"folder"> \| undefined` | `RecordId<"user"> \| undefined` |
| F8 | same with `meta::tb(…)` | `RecordId<"user"> \| RecordId<"folder"> \| undefined` | `RecordId<"user"> \| undefined` |
| F9 | `… IF !$x THEN THROW 'x' END; RETURN $x.name;` | `unknown` | `string` |
| F10 | `… IF type::is_none($x) THEN THROW 'x' END; RETURN $x.name;` | `unknown` | `string` |
| F11 | `… IF type::is_object($x) THEN RETURN $x.name END; THROW 'e';` | `unknown` | `string \| undefined` |
| F12 | `LET $o = <any> 1; IF type::is_int($o) THEN RETURN $o + 1 END; RETURN 0;` | `unknown` | `number` |
| F13 | `SELECT name, email FROM user WHERE email` (truthiness) | `Array<{ email?: string; … }>` | `Array<{ email: string; … }>` |
| F14 | `SELECT name, email FROM user WHERE !(email = NONE)` | `Array<{ email?: string; … }>` | `Array<{ email: string; … }>` |
| F15 | `SELECT name, email FROM user WHERE type::is_string(email)` | `Array<{ email?: string; … }>` | `Array<{ email: string; … }>` |
| F16 | `SELECT status FROM user WHERE status = 'active'` (`TYPE 'active'\|'inactive'\|'banned'`) | `"active" \| "inactive" \| "banned"` | `"active"` |
| F17 | `SELECT status FROM user WHERE status IN ['active','inactive']` | `"active" \| "inactive" \| "banned"` | `"active" \| "inactive"` |
| F18 | `SELECT status FROM user WHERE status = 'active' OR status = 'banned'` | `"active" \| "inactive" \| "banned"` | `"active" \| "banned"` (the `Any`-join rule of §2.6) |
| F19 | `SELECT status FROM user WHERE status != 'banned'` | unchanged | `"active" \| "inactive"` |
| F20 | `LET $names=['a','b']; LET $n=…; IF $names CONTAINS $n THEN …` | not narrowed | narrowed to `string` (one atom, three spellings) |
| F21 | same with `INSIDE` | not narrowed | narrowed |
| F22 | `… IF $n IN ['a','b'] THEN …` (array **literal**, not a param) | not narrowed | narrowed |
| F23 | `LET $rows = (SELECT name, email FROM user); RETURN $rows.filter(\|$r\| $r.email != NONE).map(\|$r\| $r.email);` | `Array<undefined \| string>` | `Array<string>` |
| F24 | `LET $x = …; LET $y = $x; IF $y = NONE THEN THROW 'e' END; RETURN $x.name;` | `unknown` | `string` (LET-alias normalization in `place_of`) |
| F25 | `SELECT * FROM ONLY user WHERE id = user:x LIMIT (1)` | W4026 + array-length lost | clean |
| F26 | `SELECT count() FROM t` written as `SELECT (count()) FROM t` | no W-code for per-row `count()` | same diagnostic as the unparenthesized form |
| F27 | `FOR $t IN $u.tags` where `tags: option<array<string>>`, past a `!= NONE` guard | loop var untyped (`NEW-4`) | `string` |
| F28 | `SELECT VALUE (email + 1) FROM user` | **no diagnostic**, `Array<unknown>` | `error[E2004]` + `warning[W2015]`, matching the unparenthesized form |
| F29 | `SELECT VALUE (name.bogusmethod()) FROM user` | **no diagnostic**, `Array<unknown>` | `error[E5001]` |
| F30 | `LET $n = (SELECT VALUE note FROM ONLY user LIMIT 1); IF $n = NONE THEN THROW 'e' END; RETURN $n;` where `note: option<string \| null>` | **`string` — unsound**, the `null` is dropped | `string \| null` |
| F31 | `IF $x.y != NONE { RETURN $x.y.len(); }` | **`error[E5001]: 'option<string>' has no method 'len'`** — FP on proven-safe code | clean |
| F32 | `$x IS NONE OR f($x)` / `($x = NONE) OR f($x)` / `$x = NONE OR $y = NONE OR f($x)` | not narrowed (the `OR` half never reaches the general engine) | narrowed |
| F33 | hover on `$x` inside `$x != NONE AND f($x)` | declared kind (no region is recorded) | narrowed kind |
| F34 | `RETURN $this.x;` at top level | `$this` demanded as a required host param | 6005, as `$value` already gets |

F1–F8, F13–F16 and F28–F30 have been reproduced verbatim against the release binary; the
remainder are direct consequences of the atom set and of the code paths cited, and each was
confirmed *not* to work today by the same method.

**F30 is the case that justifies the whole proposal.** It is not lost precision — it is a
generated TypeScript type that the database can violate, produced because two hand-written
recognizers for one fact drifted apart. Under §2.5 both are `subtract(k, Kind::None)`.

---

## 5. What it deliberately will not know

Stated up front so the boundary is a decision rather than a later discovery. Each of these is
either undecidable, unsound to guess, or out of scope for a static analyzer with no engine.

1. **Values, as opposed to kinds.** The domain is `Kind`. `age > 18` proves `age` is not
   NONE/NULL; it does **not** produce a range type, because `Kind` has none. `WHERE age > 18
   AND age < 10` is unsatisfiable and will still type as `number`, not bottom — the atoms are
   not solved against each other. (Deliberate: an SMT-shaped satisfiability check is a
   different project and would be the first thing to introduce unsoundness under
   integer/decimal/duration coercion.)
2. **Loop fixpoints.** A `FOR` body's effect on a place across iterations is not iterated.
   A narrowing established inside a loop body does not survive to the next iteration, and a
   place mutated in a loop is not widened to a fixpoint — it is invalidated. This is the
   documented cost of declining the CFG (§3.1). Practical impact is small: SurrealQL has no
   assignment to `LET` bindings, so the only mutation is rebinding in an inner scope.
3. **Join-point narrowing.** `IF c { LET $y = 1 } ELSE { LET $y = 'a' }` does not produce
   `$y : int | string` at the join — scopes end at the branch. Likewise a place narrowed
   differently in both branches of an `IF` whose branches both fall through is *not* joined;
   it reverts to the declared kind. Sound (widening), incomplete.
4. **Aliasing beyond syntactic `LET` chains.** `place_of` resolves `LET $y = $x` (F24), but
   not `LET $y = fn::identity($x)`, not `$obj.a` and `$obj['a']` as the same place unless the
   index is a constant, and not two places proven equal by a `WHERE a = b` predicate.
   Equality of *places* is syntactic-after-normalization; equality of *values* is not tracked.
5. **Anything behind a call the analyzer does not model.** A user-defined function's
   parameter is not narrowed by its callers' guards, and a builtin with no signature entry
   yields `Term::Opaque { kind: None }`. `Guard::Unknown` is the correct answer and the layer
   returns it.
6. **Truthiness beyond the option markers.** `Atom::Truthy(p)` refines `p` by removing NONE
   and NULL. It will **not** additionally remove `false`, `0`, `''` or `[]` from a general
   kind, because `Kind` cannot express "string except empty". It *will* remove them from a
   literal union (`meet` is exact there). Stated so nobody later reads `WHERE email` as
   proving `email` is non-empty.
7. **`OR` where the disjuncts refine different places.** `WHERE a != NONE OR b != NONE`
   proves nothing about `a` and nothing about `b`, and the layer will say so. Only a place
   refined by *every* disjunct is refined (§2.6). This is the one place where being more
   clever is tempting and wrong.
8. **Permissions and row visibility.** Out of scope here; the widening side lives in
   `docs/plans/2026-07-24-narrowing-result-types.md` §3.2 and is unaffected by this proposal.
9. **Cross-statement flow through the database.** `CREATE user SET email = 'a'; SELECT email
   FROM user` does not narrow the second statement. The analyzer does not model the store.
10. **Non-constant indices as places.** `$x[$i]` is not a `Place`. `$x[0]` is, but
    `Atom::refine` on an `Index` step only narrows a fixed-length tuple kind
    (`KindLiteral::Array`), never `array<T>` — narrowing element 0 of a homogeneous array
    would silently claim something about a kind shared by every element.

---

## 6. Staged migration

This cannot land as one commit — it touches the type of every narrowing in the corpus. Six
stages, each independently revertible, each with an explicit "proven not to lose precision"
criterion.

The governing rule for every stage: **`precision.snap` and `any_baseline.txt` must be
regenerated only with a human-read diff, and the diff must be all-improvement.** The harness
was built (commits `b9a0b70`, `8321c4a`) for exactly this refactor.

### Stage 0 — extend the corpus first (no production code)

The current corpus cannot detect this refactor's regressions: `tests/corpus/queries/30_narrowing.surql`
contains **only the spellings that work today** (`= NONE`, `IS NOT NONE`, `??`,
`IF … THEN … ELSE`), and a grep for `!= NONE|= NONE|type::table|type::is_|IN |CONTAINS`
across the whole corpus returns **3 lines**. A refactor could delete
`row_order_effect` outright and the snapshot would not move.

Add, as *current-behaviour* snapshot entries (several of which will be recorded as the wrong
answer, then flip in later stages — which is the point):

- `31_narrowing_spellings.surql` — for each fact, every equivalent spelling side by side:
  `= NONE` / `IS NONE` / `type::is_none` / `!x`; `!= NONE` / `IS NOT NONE` / `NOT (x = NONE)`;
  `type::table` / `record::tb` / `meta::tb` / `type::is_record(x,'t')`;
  `IN` / `CONTAINS` / `INSIDE`; each with and without enclosing parentheses.
- `32_narrowing_where.surql` — `WHERE` forms: bare truthiness, `!( … )`, `IN [lit,…]`,
  `OR` of two literal-eqs, literal-union field eq, `> lit` and `< lit`, both operand orders,
  aliased and `VALUE` projections.
- `33_narrowing_places.surql` — place shapes: `$x`, `$x.f`, `$x.f.g`, `$x[0]`, `$x.f[0].g`,
  bare row field, nested row field, closure parameter, `FOR` variable.
- `34_guard_verdicts.surql` — dead-branch cases: redundant re-check after a guard, exhaustive
  table-union branches, a defensive `IF $p = NONE` on a non-optional param (must **stay**
  live — this is the false-positive guard for C1).
- `35_const_folding.surql` — `LIMIT (1)`, `LIMIT 1 + 0`, `IF (1 = 1)`, `IF { true }`,
  a `DEFINE FIELD … ASSERT` fold, so the two folders' merge is pinned.
- Add three `sale`-shaped aggregate rows to `50_aggregates.surql` covering `(math::sum(x))*2`,
  `[math::sum(x)]`, `{ s: math::sum(x) }`.

Also add to `crates/workspace/tests/` a **spelling-equivalence test**: a table of
`(canonical, [equivalent spellings])` asserting that all spellings produce the *identical*
response kind. This is the test that would have caught every bug in §1.1 and §1.4, and it is
worth writing even if the rest of this proposal is deferred.

*Exit criterion:* snapshot grows; no existing line changes; `cargo test -p surrealguard-workspace`
green; oracle count unchanged.

### Stage 0.5 — fix the parenthesis lowering (10 lines, independently valuable)

`(expr)` should lower to `expr`, not to `Expr::Subquery(Statement::Expr(expr))`. A subquery
that wraps a bare expression carries no semantics that the expression does not; the
`Statement::Expr` wrapper exists only because `SubQuery` and a parenthesized expression share
a grammar node (`grammar.js:1831`). Change `Expr::subquery` (`crates/syntax/src/lower/expr.rs:359`)
to return the inner expression when the lowered statement is `Statement::Expr`.

**This one change fixes P1'–P7', F1–F6, F25, F26, F28 and F29 on its own** — including the two
error-severity false positives — and it is worth landing whether or not the rest of this
proposal proceeds. It is listed as a stage rather than a footnote because it must land
*before* Stage 3, or the dual-path comparison in Stage 3 will attribute its improvements to
the fact layer.

It is **not** a substitute for the rest: it fixes none of F7–F24 or F30–F34, and it does
nothing about the duplicated recognizers, the NULL unsoundness, or the ~189 other shape-match
sites. It removes one cliff; the design removes the category.

*Exit criterion:* the parenthesized rows added in Stage 0 flip; the unparenthesized rows do
not move; oracle count unchanged or lower.

### Stage 1 — `meet` / `subtract` on `Kind`, behind the existing API

Land §2.5's lattice in `crates/workspace/src/kinds.rs` with property tests, then reimplement
`narrow_kind` (`narrow.rs:717`) *in terms of it* while keeping its signature. No consumer
changes.

Resolve the NULL divergence of §1.5 here too, since `subtract` makes the two paths one
function whether the decision goes toward `strip_variant` or `narrow_out_none` — see §8(3).

*Exit criterion:* `precision.snap` diff contains only the §1.3 literal-union improvements
(F16, F19) and the F30 correction, read and accepted line by line. `any_baseline.txt` may
only fall. This stage is where `status = 'active'` starts working, and it is deliberately
first because it is the one change with no syntactic component — it isolates the value-side
change from the spelling-side change so a later regression can be attributed. **Note that
F30's fix registers as a *widening* in the snapshot**, which is normally the signature of a
regression; say so in the commit message.

### Stage 2 — `place_of`, `eval`, `Term`, behind the existing API

Land Layer 1. Reimplement `guard_path_of` (`:559`), `row_field_path` (`:284`),
`plain_field_segments`' narrowing uses, `collection_element_kind` (`:494`),
`string_literal` (`:643`), `eq_literal_kind` (`:317`), `type_table_path` (`:619`),
`simple_idiom_path_key` (`infer.rs:453`), and the four NONE/NULL literal recognizers of §1.5
in terms of `place_of`/`eval`. Merge the four constant folders:
`schema/define/field.rs::fold_const` becomes `eval` with `$value` bound;
`const_eval::const_eval`, `select.rs::literal_limit` and `pipeline.rs:842` become wrappers.

`place_of` also sees through `Expr::Subquery(Statement::Expr(..))` defensively, so the layer
is correct whether or not Stage 0.5 landed.

*Exit criterion:* nothing moves that Stage 0.5 did not already move, except `LIMIT 1 + 0`-style
folds. The corpus additions from Stage 0 make this a targeted diff rather than a wall.

### Stage 3 — `Guard` + `Atom` + `Facts`, with `narrow.rs` as an adapter

Land Layer 2 and 3's interpreter. Keep `positive_effects`/`negative_effects`/`where_effects`
as **adapters** that call `guard_of(..).facts(..)` and convert `Facts` back into
`Vec<Effect>` / `Vec<RowEffect>`. No consumer file changes at all.

This is the stage where old and new coexist, and it is the one that must be gated: land it
behind a `cfg`-free runtime constant `USE_FACT_LAYER: bool` in `narrow.rs`, with the old
recognizers still compiled. Run the harness **both ways** in CI for the duration of Stage 3:

```
SG_FACT_LAYER=0 cargo test -p surrealguard-workspace   # old path, snapshot must match
SG_FACT_LAYER=1 cargo test -p surrealguard-workspace   # new path, snapshot may improve
```

with a dedicated test that analyzes the corpus under both settings and asserts the new path's
kind at every site is **equal to or a subtype of** the old path's. That assertion — *never
wider, sometimes narrower* — is the mechanical statement of "no precision lost", and it is
checkable because `meet`'s subtype relation already exists from Stage 1.

*Exit criterion:* the both-ways test passes; F7–F15, F17–F22 flip; the old path's snapshot is
byte-identical to Stage 2's.

### Stage 4 — cut consumers over, one per commit, C8 first

Order matters. Take them **least-blast-radius first**, so the riskiest one lands with the
most evidence behind it:

1. **C8** (SELECT `WHERE`, `select.rs:2191`) — affects result types only, never a diagnostic.
   Delete the 11 `where_effects` functions. Also removes the `stmt.value` and aliased-projection
   restrictions, since `PlaceRoot::RowField` + a `KindOracle` over the projected object can
   key an aliased leaf.
2. **C6/C7** (`AND` short-circuit) — scoped to one expression, discarded after.
3. **C2/C3** (branch bodies) — this is where `record_narrowing` fires, so hover moves here.
   Ship with the LSP editor-surface tests (`crates/lsp/tests`, commit `03a87a3`) extended to
   cover a narrowed hover at an occurrence after a parenthesized guard.
4. **C4/C5** (fall-through) — the two statement-sequence levels, together, since they share
   `apply_fall_through_narrowing`.
4b. **Checking-side reads** (`check.rs:300-340`, `infer.rs:335-356`) — route the position
   contracts through `KindOracle::kind_of`, with longest-prefix lookup of `narrowed_path`.
   This is a *removal* of false positives (F31), so it can only lower the oracle count; but it
   is the one place where a wrong `Place` equality would suppress a **true** finding, so it
   ships with negative tests asserting that `IF $a.y != NONE { RETURN $b.y.len(); }` still
   reports.
5. **C1/C9** (dead-branch verdicts) — **last**, because it is the only consumer that can
   produce a *new diagnostic on valid code* (greying a live branch is a real false positive;
   `narrow.rs:838-842` says so). It ships with the `34_guard_verdicts.surql` corpus file and
   with `Atom::decide` gated: only atoms explicitly marked decidable (`IsNone`, `IsNotNone`,
   `IsNull`, `IsNotNull`, `InTables`, `NotInTables`, `Eq`, `NotEq`) may return a definite
   verdict; `Truthy`, `Ord`, `Member`, `HasKind` return `Unknown` in stage 4 and are enabled
   individually later, each with its own corpus case.

*Exit criterion per commit:* oracle count over `/Users/drewridley/Documents/Projects/workshop/database`
unchanged or lower; `precision.snap` diff read and all-improvement; `any_baseline.txt` only falls.

### Stage 5 — the satellite consumers

Now that the layer exists and is proven, retire the duplicated shape-matching:
aggregate promotion (fixes F3, F26), `ONLY` cardinality (fixes F4, F25),
`resolve_from_table`'s parenthesized-source hole (fixes F5), `FOR` element kind (F27),
`.filter()` closure narrowing (F23), `literal_limit`. Also in scope here, because they are
the same class of defect and the layer makes them one-liners:

- collapse the five special-param name lists (§1.7 item 7) into one binding table keyed by
  `PlaceRoot::Param`, fixing `$this` (F34), `$parent`, `$self` and `$scope`;
- replace `fn_body_branches`' text grep (`pipeline.rs:957`) with `block_diverges` over the
  lowered body — **carefully**, since `docs/plans/2026-07-25-analyzer-gap-backlog.md:645`
  records that the carve-out is load-bearing for the workshop oracle;
- make `block_diverges` (`block.rs:267`) inspect the whole statement sequence rather than only
  the last statement, and teach `statement_diverges` about a `Statement::Expr` wrapping a
  diverging block;
- give the top-level statement loop (`pipeline.rs:398-468`) a `Flow`, so top-level `RETURN`s
  contribute an exit-set type and 4006 fires after a top-level `THROW`.

Each is a separate commit with its own corpus rows. These are pure wins with no coexistence
problem, because they are *adding* a call to a proven layer rather than replacing one.

### Stage 6 — delete

Remove `narrow.rs`'s recognizers, `RowEffect`, `Effect`, `Narrowing`, `GuardPath`,
`const_eval.rs`'s folder, `field.rs`'s `ConstVal`/`fold_const`/`const_eq`, and the
`SG_FACT_LAYER` switch. The `Verdict` enum and `Reachability`/`BranchReach` survive unchanged —
they are policy, not recognition.

---

## 7. Risk

### 7.1 Ranked

| Risk | Likelihood | Blast radius | Mitigation |
|---|---|---|---|
| **A wrong `Atom::refine` is now wrong in every consumer at once** (the §3.5 concession) — *this is the single biggest risk in the proposal* | medium | **highest** — a bad refinement is simultaneously a wrong generated TS type, a wrong hover, a wrongly-greyed branch, and potentially a false E2001/E5002. Today the same bug in `row_order_effect` structurally cannot reach `guard_verdict` | §7.3's per-atom property tests are the *primary* mitigation and are non-optional; plus consumer capability gating in Stage 4, C1 cut over last, and the dual-path subtype assertion of Stage 3. Note the counter-evidence: §1.5's F30 is a soundness bug that exists *because* the recognizers are separate — separation is not safety, it is only unaudited blast radius |
| **Newly-narrowed types surface diagnostics that were previously suppressed by imprecision** | **high** — this is expected, not hypothetical | medium: e.g. once `WHERE status = 'active'` narrows to `"active"`, a downstream `kind_is_assignable_to` against `string` may fire; TI-2's note records exactly this pattern for link traversal | Stage 4 commits are one-consumer-at-a-time and gated on the workshop oracle count; a rise in oracle count blocks the commit until each new finding is adjudicated |
| **Precision *increase* breaks downstream TS consumers** | medium | medium — `email: string` where users' code handled `string \| undefined` is a compile change in their repo | narrowing only ever *removes* cases from a union, which is a safe direction for a consumer that already handled the wider type; note it in the changelog, it is not preventable |
| **`meet` is wrong for an exotic `Kind`** (nested `Either` of `Literal(Array)`, `Geometry` variants, `Kind::Function`) | medium | high — silently narrows to something the runtime can exceed | `meet` returns `None` (caller keeps the input) for every pair it does not have an explicit rule for. Default is *no refinement*, never *guessed refinement* |
| **`place_of` over-normalizes and merges two distinct locations** | low | **highest** — a refinement applied to the wrong place is unsound in a way no widening can rescue | `Place` is `Ord + Hash` and its equality is structural; `place_of` refuses anything it cannot prove is a fixed location (no method calls, no non-constant indices, no `Optional` parts, no graph steps). Aliasing resolution follows only syntactic `LET $y = $x` chains and stops at the first non-place RHS |
| **Performance regression** | low–medium | medium | §7.4 |
| **Positional narrowing (`NarrowingAnalysis`) breaks or shifts ranges** | low | medium — a visible editor regression on every hover | `Place::key()` is byte-identical to `GuardPath::key()`; the `region` computation in `if_else.rs`/`block.rs` is not touched; the LSP editor-surface tests (`03a87a3`) are the gate |
| **Stage 3's dual-path CI doubles corpus test time** | certain | low | temporary; ~0.4 s → ~0.8 s per corpus run |

### 7.2 What specifically could regress, concretely

- **`Verdict::AlwaysTrue` on a guard the const path used to leave `Unknown`.** `guard_of`
  lowers `IF 1 = 1` to `Guard::True`, but it *also* now lowers `IF (1 = 1) AND $x != NONE`
  to `All([True, IsNotNone])` — which the old `binary_verdict` explicitly refused to
  decompose (`narrow.rs:894-897`). If `$x` is flow-narrowed to non-none, the new path says
  `AlwaysTrue` and kills the `ELSE`. That may be correct, but it is *new* behaviour on
  existing corpus code and must be adjudicated, not accepted.
- **`Atom::Truthy` refining more than the old `!= NONE`.** `WHERE email` currently narrows
  nothing (§1.4). Making it narrow is correct per the engine (`is_truthy`), but every
  existing query with a bare-field `WHERE` changes shape at once. This is why `Truthy` is in
  the deferred-decidability set.
- **`Any` normalization.** `Guard::facts` must never refine a place whose current kind is
  `Kind::Any` to something narrower on the strength of an atom that does not *prove* it —
  `in_effect` guards this today (`narrow.rs:481`) and `narrow_kind`'s `Is` arm guards it again
  (`:740`). The lattice makes `meet(Any, k) = k`, which is *stronger* than today. For
  `HasKind` that is right (`type::is_int($o)` on an `any` genuinely proves `int`). For
  `Member` it is right only when the collection's element kind is concrete. The double guard
  must be preserved as an explicit precondition on `Atom::Member::refine`, not lost in the
  meet.
- **The `field_path_guard_does_not_narrow_a_sibling_path` invariant.** `apply_effects_over`
  (`:670`) is careful that narrowing `$x.a` never touches `$x` or `$x.b`. A map keyed by
  `Place` preserves this by construction, but the *application* step must keep using
  `define_narrowed_path` for non-bare places rather than rebinding the root. The
  longest-prefix lookup added in Stage 4b (§1.6 item 2) is the one change that could break it:
  it must match on a **whole-segment** prefix, so that a narrowing of `$x.ab` never applies to
  `$x.abc`.
- **Fixing the NULL divergence (F30) widens types.** `subtract(k, Kind::None)` is *less*
  narrowing than today's param path, so `$n` becomes `string | null` where it was `string`.
  That is a correctness fix, but it will register in `precision.snap` as a **widening**, which
  is normally the signature of a regression. It must be called out in the Stage 1 commit
  message, or a future reader will read the diff as a loss.
- **Retiring `fn_body_branches`' text grep.** The backlog records the carve-out as
  load-bearing for one real workshop file (`organization/organization_unit.surql:44`). A
  correct AST-based replacement may classify it differently and raise the oracle by one. That
  is a product decision, not a refactor decision, and must be taken before the commit lands.

### 7.3 The mechanical safety argument

Three layers, in increasing generality:

1. **Per-atom property tests of the two invariants in §2.5.** For a fixed finite universe of
   kinds (the ~40 that appear in the corpus, plus constructed `Either`/`Record`/`Literal`
   combinations) and a fixed finite universe of values, assert for every `(atom, kind)` pair:
   `refine(k) ⊑ k`, and every value of `k` satisfying the atom is in `refine(k)`; and
   `decide(k) = AlwaysFalse ⟹` no value of `k` satisfies it. This is the *only* place
   soundness is proven, and it is proven once rather than re-argued in 13 doc comments.
2. **The spelling-equivalence table** (Stage 0). `(canonical, [equivalents])` → identical
   response kind. Catches every §1.1/§1.4-class bug by construction, including future ones.
3. **The dual-path corpus assertion** (Stage 3). New kind at every site is a subtype of, or
   equal to, the old kind. Mechanically enforces "no precision lost" across the whole
   migration rather than per-commit eyeballing.

Plus the two existing harnesses used as designed: `precision_snapshot.rs` catches any type
that moves; `any_ratchet.rs` catches any site that becomes imprecise. Both are additive-only
and both fail loudly, which is exactly the property a refactor of this size needs. Neither is
sufficient alone — `precision.snap` only covers the 14-file corpus, which is why Stage 0
comes first.

### 7.4 Performance

Baseline, measured on this machine: the release binary over the 149-source workshop
(`/Users/drewridley/Documents/Projects/workshop/database`) takes **0.42–0.45 s wall**
including process start, three runs. The commit message for `6f614f0` records 300 ms for the
analysis itself.

Where cost is added:

- `place_of` and `eval` allocate a `Place` (a `String` + a `Vec<Step>`) where the old code
  allocated a `GuardPath` (a `String` + a `Vec<String>`) — a wash.
- `guard_of` builds a `Guard` tree per guard expression, where the old code built a
  `Vec<Effect>` — one extra small allocation per guard, bounded by the number of `AND`/`OR`
  nodes. Guards in the corpus are 1–3 leaves.
- `Facts` is a `BTreeMap<Place, Kind>` where the old code returned a `Vec<Effect>` — slightly
  more expensive for 1–2 entries, and cheaper for the `AND`-of-many case because it
  deduplicates.
- `meet`/`subtract` recurse over `Kind`, which the old transforms also did.
- Stage 5 *adds* calls (aggregate promotion, `ONLY` cardinality, `FOR`) that previously
  short-circuited on a shape mismatch. These now do real work more often.

**Estimate: +5–8% on full analysis (≈ +15–25 ms on the workshop), dominated by Stage 5's
newly-reached code paths rather than by the layer itself.** The layer is called once per
guard expression, and guard expressions are a small fraction of the AST; parsing and schema
resolution dominate. Two things must be measured, not assumed, and belong in the Stage 3
commit message: the dual-path run (which is 2× by construction and temporary), and the
symbol-level invalidation path (`76e591d`), because a `Place`-keyed map is a new input to the
dependency tracking if narrowing ever feeds it.

If the estimate is exceeded, the first lever is memoizing `place_of` per `Spanned<Expr>` span
within a statement — guards are re-walked by `guard_verdict` *and* `positive_effects` *and*
`negative_effects` today, three times over the same subtree, which the single `Guard`
construction already removes.

---

## 8. Open questions for the author

1. **`Atom::Truthy`.** Turning `WHERE email` into a narrowing is engine-correct but changes
   every bare-field `WHERE` in every consumer's codebase at once. Ship it, or leave it
   `Unknown` and revisit?
2. **`OR` refinement (F18).** `docs/plans/2026-07-24-narrowing-result-types.md` §3.1 defers
   this to P2. The `Guard::Any` join rule gives it for free. Take it in Stage 4, or hold?
3. **F30 (the NULL unsoundness) should be fixed now, not in Stage 1.** It is a wrong
   generated type shipping today, it is a two-line change (`narrow_kind`'s `NotNone` arm
   calling `strip_variant` instead of `narrow_out_none`, or the reverse decision made
   deliberately), and it needs a product call: is `!= NONE` on an `option<T | null>` meant to
   prove non-null? The engine says no. Confirm and fix independently of this proposal.
4. **Grammar-level gaps this design cannot fix.** `NOT x` lowers to `PrefixOp::Other("NOT")`
   and the grammar rejects `IF NOT true` outright (`S0001`) — while still folding the bare
   `true` and reporting `W4024` on the ***wrong* branch**, which is a live soundness bug in
   the reachability analysis and should be filed separately. `FOR $t IN $u.tags` is a grammar
   rejection (SX-2). Stage 0.5's parenthesis lowering fix is the counterpart in the other
   direction: a ten-line change that resolves the largest cliff in §1 on its own.
5. **Does the `KindOracle` trait belong on `StatementEnv` instead?** `StatementEnv` already
   answers `let_fact` / `narrowed_path` / `is_param_narrowed`. Making it *the* oracle avoids a
   trait, but forces the SELECT row post-pass and the closure-element scope to construct a
   `StatementEnv`, which they currently do not. The trait is proposed because those two
   consumers are the point of the exercise — but it is a judgement call, not a requirement.
</content>
</invoke>
