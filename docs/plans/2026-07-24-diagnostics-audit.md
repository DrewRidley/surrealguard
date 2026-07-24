# SurrealGuard diagnostics audit (2026-07-24)

# SurrealGuard Lint Catalog — Tech Lead Synthesis

Markers: **[R]** = registered/reserved but no emit site today · **[NEW]** = proposed lint, not yet in catalog · **†** = default-level change vs. today's policy.

---

## 1. Default-Level Table (all codes, grouped by level)

### DENY — contract violations (query fails or is provably wrong at runtime)

| Code | Description | Default |
|------|-------------|---------|
| 1001 | Reference to an undefined table | deny |
| 1002 | Undeclared field on a SCHEMAFULL table | deny |
| 1012 | REBUILD/REMOVE INDEX names a non-existent index/table | deny |
| 1025 **[R]** | Subfield declared under a scalar (non-object) parent — dead | deny |
| 1027 | @@/vector operator without a backing SEARCH/MTREE/HNSW index | deny |
| 1032 | DEFINE ANALYZER names an unknown tokenizer/filter/language | deny |
| 2001 | Value does not inhabit the field's declared type | deny |
| 2004 | Incompatible operands (arithmetic/comparison) | deny |
| 2007 | Cast to an unknown type name | deny |
| 2008 | Cast that provably cannot succeed | deny |
| 2012 | fn::/closure body returns a type outside its declared return | deny |
| 2017 | ORDER BY names a field not on the result rows | deny |
| 2018 | LIMIT/START non-integer or negative | deny |
| 2019 | TIMEOUT non-duration | deny |
| 2020 | KILL requires a uuid | deny |
| 2021 | SHOW SINCE non-versionstamp/datetime | deny |
| 2022 | FOR over a non-iterable scalar | deny |
| 2025 | Writing a READONLY field after creation | deny |
| 2030 | Index/filter/splat on a non-collection | deny |
| 2031 | Uncompilable regex literal | deny |
| 2032 | Invalid datetime/duration/uuid literal content | deny |
| 2033 | Malformed PATCH op/path on a constant payload | deny |
| 2034 | Required (non-optional, no-DEFAULT) field omitted at creation | deny |
| 2035 | Invalid analyzer filter args (edgengram/ngram bounds) | deny |
| 2036 | GeoJSON literal with unknown `type` | deny |
| 2037 | DEFAULT provably violates its own ASSERT | deny |
| 3001 | ->/<- traversal from a non-relation table | deny |
| 3002 | Wrong-direction/wrong-endpoint relation traversal | deny |
| 3009 | Traversal from a field that holds no records | deny |
| 4001 **[R]** | Invalid clause on statement (parser-covered) | deny |
| 4002 **[R]** | `SELECT VALUE a, b` (parser-covered) | deny |
| 4003 | ONLY on a table-wide (multi-row) target | deny |
| 4004 | INSERT column/value count mismatch | deny |
| 4005 | BREAK/CONTINUE outside any FOR loop | deny |
| 4007 | Transaction pairing error (unopened/nested/unclosed) | deny |
| 4009 **[R]** | LIVE SELECT with GROUP/ORDER/LIMIT/SPLIT (blocked by lowering) | deny |
| 5001 | Call to a function that does not exist | deny |
| 5002 | Function arity / argument-kind mismatch | deny |
| 5005 | Const arg provably violates the fn's value contract | deny |
| 6001 | Irreconcilable constraints on one param | deny |
| 6005 | $before/$after/$value/$event/$input used outside binding context | deny |
| 6006 **[R]** | Host-declared param type contradicts query requirement | deny |
| 6007 | LET assignment to a protected/context param | deny |
| 8001 **[R]** | Function unavailable in the target SurrealDB version | deny |
| 8003 **[R]** | Syntax requires a newer target SurrealDB version | deny |

### WARN — executable but suspicious (advisory, on by default)

| Code | Description | Default |
|------|-------------|---------|
| 1021 | REMOVE of a non-existent table/field (no-op) | warn |
| 1022 | DEFINE without OVERWRITE silently replaces a prior definition | warn |
| 1023 | FETCH on a scalar (silent no-op) | warn |
| 1024 | SPLIT on a non-collection (row unchanged) | warn |
| 1029 | Two indexes over the same field set (redundant write cost) | warn |
| 2005 | Non-bool WHERE/ASSERT/IF (truthiness-tested) | warn |
| 2015 | Arithmetic on a possibly-NONE operand | warn |
| 2026 | Assignment to a VALUE-computed field (write discarded) | warn |
| 3004 **†** | FROM lands on an edge instead of the target node | warn *(was deny)* |
| 3011 | Unbounded graph recursion (`{..}`/`{1..}`) | warn |
| 4006 | Unreachable statement after divergence | warn |
| 4010 | Duplicate SET target (last write wins) | warn |
| 4012 **[R]** | OMIT without `*` projection (parser-covered) | warn |
| 4013 **[R]** | GROUP BY key not in projections | warn |
| 4017 | Value-position block ending in LET (evaluates to NONE) | warn |
| 4019 | CREATE/INSERT on a relation table without in/out | warn |
| 4020 | RETURN BEFORE on CREATE (always NONE) | warn |
| 4021 **†** | SHOW CHANGES on a non-changefeed table (reads empty) | warn *(was deny)* |
| 4022 | SELECT from a DROP table (never returns rows) | warn |
| 4023 | count() without GROUP (per-row, not total) | warn |
| 5009 | fn:: unconditional non-termination | warn |
| 5010 **[R]** | Event self-trigger / trigger cycle | warn |
| 6002 **[R]** | LET shadows a DEFINE PARAM of a different kind | warn |
| 6004 | Param read before its own LET runs | warn |
| 7001 **[R]** | Unused LET binding | warn |
| 7004 | Branch/loop decided by a constant | warn |
| 7005 | Value-proven always-false/true comparison | warn |
| 7006 | Provably-empty / kind-disjoint membership test | warn |
| 7007 | Redundant wildcard (`SELECT *, age`) | warn |
| 7011 | SET on immutable `id` | warn |
| 7012 | Blocking/side-effecting call in a computed context | warn |
| 7013 | Malformed suppression directive | warn |

### ALLOW — opt-in perf/style opinions (off by default)

| Code | Description | Default |
|------|-------------|---------|
| 4016 **[R]** | Empty block in value position (style) | allow |
| 6003 **†** | SurrealGuard cannot analyze a type yet (tool-coverage note) | allow *(was hint/on)* |
| 7002 **†** | Variable shadowing | allow *(was warn)* |
| 7003 **†** | Heterogeneous array literal | allow *(was warn)* |
| 7008 | Schemaless table in a typed workspace | allow |
| 7009 | Whole-table UPDATE/DELETE without WHERE | allow |
| 7014 **[NEW]** | Whole-table SELECT with no WHERE and no LIMIT | allow |
| 7015 **[NEW]** | Any `SELECT *` (over-fetch / schema-drift brittleness) | allow |

---

## 2. Allow-by-Default (Opt-In) Set — explicit

These eight are the **only** codes off by default. Everything else is deny or warn. The line is: a code is `allow` **only** if a competent team could deliberately want the flagged construct in production.

- **7002** — shadowing (legitimate, common; clippy keeps its shadow lints allow-by-default) **†**
- **7003** — heterogeneous arrays (`array<int|string>` is real) **†**
- **7008** — schemaless-in-typed-workspace (product-owner mandate) 
- **7009** — whole-table UPDATE/DELETE (bulk migrations)
- **6003** — SurrealGuard's own analyzer-coverage gap; noise for most users, useful only to those auditing tool coverage **†**
- **4016 [R]** — empty value-position block, pure style (if ever revived)
- **7014 [NEW]** — whole-table SELECT without WHERE/LIMIT (read-side analogue of 7009; small tables & admin queries legitimately hit it). FP risk: medium.
- **7015 [NEW]** — any `SELECT *` (pure precision/perf opinion; `SELECT *` is extremely common and often fine). FP risk: high. **Keep strictly separate from 7007**, which stays warn and only fires on the redundant `SELECT *, field` overlap.

Note the two new lints share one trigger (`SELECT * FROM person`) but split by intent: 7014 keys on *no WHERE/no LIMIT* (perf), 7015 on *wildcard projection* (precision). A bare `SELECT * FROM person` with the opt-in set fully enabled would trip both — acceptable since each names a distinct fix.

---

## 3. Message-Rewrite Punch List (ranked, weak copy only)

Ranked by user impact. Excludes unimplemented codes (see §4 for their prospective copy).

1. **5005 — Rust Debug leak (real bug).** `field.rs:30` renders a `surrealdb_types::Value` with `{other:?}`, so users see `Number(Int(42))` instead of `42`. Rewrite consequence-first with Display: `type::field needs a field-path string, but this argument is the {kind} \`{value}\`` + help `pass a string naming a field, e.g. type::field('name.first')`. **Never use `{:?}` on a Value anywhere.**

2. **5001 — builtin path has no did-you-mean.** The fn:: site suggests via `suggest::closest`; the builtin fallthrough (`unknown_function`) does not. A typo'd builtin (`string::lenght`) is the single most common case and gets zero fix hint. Run `suggest::closest` over builtin names → help `did you mean \`string::length\`?`, else `no builtin or DEFINE FUNCTION named \`...\` exists`.

3. **5002 — builtin sites lack help + stale doc numbers.** The fn:: site is exemplary; the two `signature.rs` builtin sites have no help line. Add: arity → `\`string::len\` signature: string::len(string) -> int`; arg-kind → `pass a \`string\``. Also fix stale doc-comments referencing retired codes 5003/5004 (`signature.rs:76`, `check_closure_arity`) → 5002.

4. **1032 — analyzer messages are bare, not consequence-first, no valid-option list.** `\`x\` is not a filter` → `DEFINE ANALYZER will fail: \`klingon\` is not a supported snowball language` + help listing valid options (languages: arabic…turkish; tokenizer: blank, camel, class, punct; filter: ascii, lowercase, uppercase, snowball, edgengram, ngram).

5. **3002 — leads with the declaration, not the consequence.** → `this step can't traverse \`likes\` from \`person\` — the relation runs \`person\`->\`likes\`->\`post\`` + help `traverse from a table on the near side, or reverse the arrow`. Keep the DEFINE TABLE `with_related` note.

6. **1022 — DEFINE TABLE variant is terse and help-less** while its DEFINE FIELD twin is exemplary. → `\`person\` is already defined; this DEFINE silently replaces the earlier one` + help `use \`DEFINE TABLE OVERWRITE person\`` + `with_related` note at the first definition (mirror the FIELD site).

7. **6005 — vague about consequence and valid context.** → `\`$before\` is not bound here, so it reads NONE` + help `\`$before\`/\`$after\`/\`$value\` exist only inside DEFINE EVENT ... THEN and field VALUE/ASSERT clauses`. Naming the binding contexts is the fix most users need.

8. **4019 — leads with the fix, not the consequence.** → `creating \`likes\` without \`in\`/\`out\` makes an edge that connects nothing` + help `use \`RELATE a->likes->b\`, or set both \`in\` and \`out\``.

9. **6004 — states ordering but not consequence.** → `\`$x\` is read here before its LET runs, so it is NONE` + help `move the LET above this use` + note anchored at the LET span.

10. **4007 — two of three transaction messages have no help.** Unopened COMMIT/CANCEL → help `remove it, or add a matching \`BEGIN\` above`; nested BEGIN → help `close the first transaction with COMMIT/CANCEL before opening another`. (The unclosed-BEGIN variant already inlines its fix.)

11. **4003 — mutation.rs emit weaker than select.rs.** Align consequence-first: `this ONLY target can match many rows, but ONLY must resolve to exactly one — SurrealDB errors here at runtime` + help `target a record id like \`person:tobie\``.

12. **5009 — awkward for mutual cycles, no fix.** Single-element cycle → `...: it calls itself with no base case`; mutual → `...: the cycle A -> B -> A has no base case`; add help `add an IF/FOR base case, or break the cycle`.

13. **7011 — no fix line.** Keep `record ids are immutable; id is set at creation` + help `choose the id when you create the record (\`CREATE person:the_id ...\`), not with SET on an existing row`.

14. **7005 / 7006 / 7004(IF) / 7003 — batch: add help lines.** 7005 → help `these kinds can never be equal, so this test is dead`. 7006 → help `this membership test can never match — remove it, or check the collection/element`. 7004 IF variant → help `the condition is always <true/false>; drop the IF and keep the live branch`. 7003 → `this array mixes element kinds, so its type widens to \`array<int | string>\`` + help `make every element the same kind if that was not intended`.

15. **1024 / 4006 — trivial help additions.** 1024 → help `SPLIT fans one output row out per element of an array/set field; a scalar has nothing to split`. 4006 → help `remove it, or move it before the RETURN/BREAK/THROW`.

Cross-cutting nits worth a code comment: the 6005 `CONTEXT_ONLY_PARAMS` set and 6007 `PROTECTED` set overlap but differ ($auth/$session/$token/$access/$parent are protected-only) — document so they don't drift.

---

## 4. Completeness-Gap List (ranked, with FP risk)

Ranked by value ÷ cost. Cheap, low-FP, high-certainty first.

1. **1025 — subfield under a scalar parent has zero emit site.** `DEFINE FIELD a TYPE int; DEFINE FIELD a.b TYPE string` — `a.b` is dead. Catalog lists 1025 (🔶) but grep confirms no emit site. Schema-only detectable (parent path resolves to a non-object, non-Any kind). **deny · FP low.**

2. **2005 on mutation WHERE.** `UPDATE person SET age = 30 WHERE age` escapes the not-bool check — only SELECT's `check_where_clause` runs it; UPDATE/DELETE/UPSERT go through `analyze_expression_positions_for`, which never applies `definitely_not_bool`. The SELECT fix already exists; mirror it. **warn · FP low.**

3. **7012 misses DEFINE EVENT bodies.** Catalog scopes 7012 to "field VALUE or event body" but `check_computed_calls` is only invoked from `schema/define/field.rs`. An `http::post`/`sleep` in a `DEFINE EVENT ... THEN` block runs on every write — identical footgun, uncaught. **warn · FP low.**

4. **math::fixed invalid constant precision.** `math::fixed(3.14159, -1)` — second arg declared `Numeric`, const value never validated. Catalog lists it as an intended 5005 case (🔶). Guard: fire only on literal negative/non-integer via `const_value_arg`. **deny · FP low.**

5. **6002 — LET shadows a DEFINE PARAM of a different kind.** `DEFINE PARAM $min_age VALUE 18; LET $min_age = 'x'`. Registered but only referenced in a policy test. DEFINE PARAM kind is known at schema time, LET kind is inferred — cheap check. **warn · FP low.**

6. **type::thing / type::table const table-name validation (5005 family).** `type::thing('prson', 42)` / `type::table('ghost')` — same contract as 1001 but silently accepted because these use `ParamKind::Any` and never inspect a const first arg. Guard: fire only on a static string literal AND in a typed/schemafull workspace (schemaless legitimately references undefined tables). **deny · FP medium.**

7. **4023 extend to scalar `math::` aggregates without GROUP.** `SELECT math::sum(amount) FROM invoice` returns per-row amounts, never the total — same trap as bare count(). `is_bare_count()` only matches count/count::count; `column_aggregate_kind()` already promotes the scalar to array (masking the confusion). Guard: fire only for a single bare **scalar** column arg — a column already `array` (`math::sum(scores)`) is a legit per-row reduction and must not fire. **warn · FP medium.**

8. **4009 — LIVE SELECT clauses can't fire (lowering discards them).** `LiveSelectStmt` (`ast/statement.rs:498`) stores only `table`; its lowering (`lower/statement.rs:104`) keeps only the table, so GROUP/ORDER/LIMIT/SPLIT never reach the analyzer. Catalog's "🔶 lowering keeps clauses" note is inaccurate — it does not. Requires extending the AST + lowering to retain clause spans, then emitting. Bigger cost than the above. **deny · FP low.**

9. **5010 — event self-trigger / trigger-cycle detection.** `DEFINE EVENT bump ON person ... THEN (UPDATE person SET n = n + 1)` recurses to SurrealDB's depth limit. The events analogue of 5009's fn:: cycle detection; reuse the three-color DFS over an event-effect graph. Guard: only flag cycles with no branching guard (bounded cascades are legit). **warn · FP medium.**

10. **7001 — unused LET.** `LET $unused = 42; RETURN 1`. Registered (env use-tracking exists) but no emit site. Guard against side-effecting RHS (`LET $_ = fn::audit()`) — fire only when the RHS is pure. **warn · FP medium.**

11. **7004 constant WHERE.** `SELECT * FROM person WHERE 1 = 1` — catalog lists it as a 7004 example but emit sites are only if_else.rs and for_loop.rs. `WHERE true` is sometimes a deliberate query-builder placeholder — guard for FP. **warn · FP medium.**

12. **8001 / 8003 — version-compat family entirely unimplemented.** No version registry, no emit sites. Every version incompatibility (function added in a later release, closure/`??` syntax) is silently missed. Largest infra lift (needs a per-version function+syntax registry) — treat as its own track, not a quick add. **deny · FP low.**

### New opt-in perf lints (product-owner requested) — separate track, all `allow`

13. **7014 — whole-table SELECT, no WHERE/no LIMIT.** Read-side analogue of 7009. Emit at `analyzer/data/select.rs` when FROM is a bare table with no where_clause, no LIMIT, no record-id target. **allow · FP medium.**

14. **7015 — any `SELECT *`.** Over-fetch / schema-drift brittleness. Emit when the sole projection is a Wildcard and the lint is opted on. Must stay distinct from 7007 (redundant-wildcard, warn). **allow · FP high.**

**Sequencing recommendation:** ship #1–5 first (cheap, low-FP, immediate correctness value), then #6–7 (guarded medium-FP), then the opt-in perf pair #13–14 (isolated, no false-positive blast radius since off by default), and treat #8 (4009 AST change), #9 (event graph), and #12 (version registry) as scoped infrastructure projects.

---

[
  {
    "family": "1xxx-schema + 2xxx-types",
    "summary": "Audited all 11 emitting 1xxx codes and 24 emitting 2xxx codes against their real emit sites (data/mod.rs, data/mutation.rs, data/select.rs, data/insert.rs, expression/check.rs, schema/define/{field,index,event,analyzer,table,function}.rs, schema/{remove,show}.rs, flow/for_loop.rs, kill.rs) and SurrealDB semantics. Message quality is strong overall \u2014 most codes are consequence-first with help: and note: (related) lines. Weak spots: 1022's DEFINE TABLE variant (\"duplicate table definition `person`\") is terse and help-less while its DEFINE FIELD twin is exemplary; 1032 analyzer messages are bare (\"`x` is not a filter\") with no consequence or help; 1024 SPLIT lacks a help line. Default-level recommendations: all schema-reference and type-mismatch/cast/arity codes are contract violations \u2192 deny, except runtime-tolerated no-ops (1021 REMOVE-ghost, 1022 redefine, 1023 FETCH-scalar, 1024 SPLIT-scalar, 1029 dup-index, 2005 non-bool cond, 2015 maybe-NONE, 2026 computed-write) \u2192 warn. Nothing in this family is a perf/style opinion that belongs at allow. Four real completeness gaps found: 1025 (subfield under scalar parent) is catalog-listed but has ZERO emit site; UPDATE/DELETE/UPSERT WHERE non-bool escapes 2005 (only SELECT emits it); plus the two owner-requested new perf lints (whole-table SELECT, bare SELECT *) which should default to allow.",
    "codes": [
      {
        "code": "1001",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "Reference to an undefined table is a near-certain bug; SurrealDB queries a table that isn't there. Note: the data/mod.rs path has good 'did you mean'/'no DEFINE TABLE' help, but the event.rs and index.rs variants omit the suggestion \u2014 worth aligning.",
        "message_rewrite": ""
      },
      {
        "code": "1002",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "Undeclared field on a SCHEMAFULL table is a contract violation (FLEXIBLE/schemaless exempt). Messages carry 'did you mean' + note-at-definition on the main path; event.rs variant lacks the suggestion.",
        "message_rewrite": ""
      },
      {
        "code": "1012",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "REBUILD/REMOVE INDEX naming a non-existent index/table fails at runtime; clear reference violation.",
        "message_rewrite": ""
      },
      {
        "code": "1021",
        "current_message_ok": true,
        "recommended_default_level": "warn",
        "level_rationale": "REMOVE of a non-existent table/field is a no-op at runtime, not a failure, but almost always a typo \u2014 advisory. Good help + related.",
        "message_rewrite": ""
      },
      {
        "code": "1022",
        "current_message_ok": false,
        "recommended_default_level": "warn",
        "level_rationale": "Silent redefinition without OVERWRITE is executable but suspicious. The DEFINE FIELD emit site is exemplary (help + related-to-first-def); the DEFINE TABLE emit site in schema/define/table.rs is terse and help-less \u2014 inconsistent.",
        "message_rewrite": "message: \"`person` is already defined; this DEFINE silently replaces the earlier one\"; help: \"use `DEFINE TABLE OVERWRITE person` to redefine it intentionally\"; note: point at the first `DEFINE TABLE person` (mirror the DEFINE FIELD 1022 site's with_related)."
      },
      {
        "code": "1023",
        "current_message_ok": true,
        "recommended_default_level": "warn",
        "level_rationale": "FETCH on a scalar is a silent no-op in SurrealDB (no runtime error), so warn fits the 'suspicious but executable' class better than error. Message is consequence-first with help.",
        "message_rewrite": ""
      },
      {
        "code": "1024",
        "current_message_ok": false,
        "recommended_default_level": "warn",
        "level_rationale": "SPLIT on a non-collection yields the row unchanged (no runtime failure) \u2014 advisory. Message states the mismatch but has no help line explaining the consequence.",
        "message_rewrite": "keep message; add help: \"SPLIT fans one output row out per element of an array/set field; a scalar has nothing to split\"."
      },
      {
        "code": "1025",
        "current_message_ok": false,
        "recommended_default_level": "deny",
        "level_rationale": "Contract violation (subfield under a scalar parent can never apply). Catalog lists it as \ud83d\udd36 but there is NO emit site \u2014 see completeness gaps. No message exists to audit.",
        "message_rewrite": ""
      },
      {
        "code": "1027",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "@@/vector operator without its backing SEARCH/MTREE/HNSW index is a hard runtime error. Excellent message: consequence + actionable DEFINE INDEX help.",
        "message_rewrite": ""
      },
      {
        "code": "1029",
        "current_message_ok": true,
        "recommended_default_level": "warn",
        "level_rationale": "Two indexes over the same field set (same kind) is genuine redundant write cost, almost never intended \u2014 a correctness-adjacent warning, not a style opinion, so it stays on by default. Good help.",
        "message_rewrite": ""
      },
      {
        "code": "1032",
        "current_message_ok": false,
        "recommended_default_level": "deny",
        "level_rationale": "Unknown tokenizer/filter/language makes DEFINE ANALYZER fail. Messages are bare and non-consequence-first ('`x` is not a filter') with no help and no list of valid options.",
        "message_rewrite": "message: \"DEFINE ANALYZER will fail: `klingon` is not a supported snowball language\"; help: \"supported: arabic, danish, dutch, english, french, german, \u2026 turkish\". Same shape for tokenizer ('supported: blank, camel, class, punct') and filter ('supported: ascii, lowercase, uppercase, snowball, edgengram, ngram')."
      },
      {
        "code": "2001",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "Value not inhabiting a field's declared type is the core type-mismatch contract. Strong messages: the NONE-into-required variant gives the actionable option<>/?? fix, others carry note-at-definition.",
        "message_rewrite": ""
      },
      {
        "code": "2004",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "Incompatible operands: arithmetic throws, comparisons are silently kind-ordered \u2014 tolerated misuse is exactly what the tool exists to surface. Clear message; the += /-= variant even adds help + related.",
        "message_rewrite": ""
      },
      {
        "code": "2005",
        "current_message_ok": true,
        "recommended_default_level": "warn",
        "level_rationale": "A non-bool WHERE/ASSERT/IF is truthiness-tested at runtime (executable), so advisory. Good help on the WHERE/ASSERT sites. (Coverage gap on UPDATE/DELETE \u2014 see gaps.)",
        "message_rewrite": ""
      },
      {
        "code": "2007",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "Cast to an unknown type name is a definite error; generous allowlist means only real misspellings fire, and it suggests the nearest name.",
        "message_rewrite": ""
      },
      {
        "code": "2008",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "A cast that provably cannot succeed (kind- or value-proven) is a runtime failure. Clear message.",
        "message_rewrite": ""
      },
      {
        "code": "2012",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "fn::/closure body returning a type that doesn't inhabit the declared return type is a contract violation. Message is consequence-first ('declares `-> string` but its body returns `int`').",
        "message_rewrite": ""
      },
      {
        "code": "2015",
        "current_message_ok": true,
        "recommended_default_level": "warn",
        "level_rationale": "Arithmetic on a possibly-NONE operand fails only when NONE shows up at runtime \u2014 advisory, and the emit site already gives the ?? coalesce fix in help.",
        "message_rewrite": ""
      },
      {
        "code": "2017",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "ORDER BY naming a field not on the result rows is a reference violation. Two message variants both clear.",
        "message_rewrite": ""
      },
      {
        "code": "2018",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "LIMIT/START non-integer or negative is a runtime error. Clear split messages ('needs an integer' / 'can't be negative').",
        "message_rewrite": ""
      },
      {
        "code": "2019",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "TIMEOUT non-duration is a type error. Clear message.",
        "message_rewrite": ""
      },
      {
        "code": "2020",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "KILL requires a uuid; wrong kind fails. Clear message.",
        "message_rewrite": ""
      },
      {
        "code": "2021",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "SHOW SINCE non-versionstamp/datetime is a type error. Clear message.",
        "message_rewrite": ""
      },
      {
        "code": "2022",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "FOR over a non-iterable scalar is a runtime error; ranges/arrays correctly exempt. Clear message.",
        "message_rewrite": ""
      },
      {
        "code": "2025",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "Writing a READONLY field after creation is a hard runtime error. Excellent message: consequence + READONLY help + note-at-definition.",
        "message_rewrite": ""
      },
      {
        "code": "2026",
        "current_message_ok": true,
        "recommended_default_level": "warn",
        "level_rationale": "Assigning a VALUE-computed field is silently overwritten (no error) \u2014 advisory. Strong message: 'this write is discarded' + help + related.",
        "message_rewrite": ""
      },
      {
        "code": "2030",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "Indexing/filtering/splatting a non-collection is a runtime error. Message names the offending kind clearly.",
        "message_rewrite": ""
      },
      {
        "code": "2031",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "An uncompilable regex literal fails. Clear message.",
        "message_rewrite": ""
      },
      {
        "code": "2032",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "Invalid datetime/duration/uuid literal content is a parse/runtime error. Clear per-kind messages.",
        "message_rewrite": ""
      },
      {
        "code": "2033",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "Malformed PATCH op/path is a runtime error on constant payloads. Clear messages.",
        "message_rewrite": ""
      },
      {
        "code": "2034",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "Omitting a required (non-optional, no-DEFAULT) field at creation fails. Excellent message: consequence + why + note-at-definition. Correctly wired for CREATE and INSERT.",
        "message_rewrite": ""
      },
      {
        "code": "2035",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "Invalid analyzer filter args (edgengram/ngram bounds) make DEFINE ANALYZER fail. Message states the required shape.",
        "message_rewrite": ""
      },
      {
        "code": "2036",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "A GeoJSON literal with an unknown `type` is malformed. Message is clear; minor: could add a 'did you mean `Point`?' suggestion like 2007 does.",
        "message_rewrite": ""
      },
      {
        "code": "2037",
        "current_message_ok": true,
        "recommended_default_level": "deny",
        "level_rationale": "A DEFAULT that provably violates its own ASSERT turns every field-omitting CREATE into a hard error. Consequence-first message; minor: no help showing the folded value, but the folder bails on anything uncertain so it's low-FP.",
        "message_rewrite": ""
      }
    ],
    "completeness_gaps": [
      {
        "title": "1025 (subfield declared under a scalar/non-object parent) has no emit site",
        "proposed_code_family": "1025",
        "default_level": "deny",
        "fp_risk": "low",
        "example_surql": "DEFINE TABLE t SCHEMAFULL;\nDEFINE FIELD a ON t TYPE int;\nDEFINE FIELD a.b ON t TYPE string;",
        "why_a_bug": "SurrealDB accepts both DEFINEs, but `a.b` can never apply because `a` is a scalar `int`, not an object \u2014 the subfield is dead. The catalog lists 1025 as \ud83d\udd36 but grep confirms zero emit sites; the contract ('a subfield is declared under an object-shaped parent') is unimplemented. Detectable from schema alone: the parent path resolves to a non-object, non-Any declared kind."
      },
      {
        "title": "WHERE/condition non-bool (2005) is not flagged on UPDATE/DELETE/UPSERT",
        "proposed_code_family": "2005",
        "default_level": "warn",
        "fp_risk": "low",
        "example_surql": "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD age ON person TYPE int;\nUPDATE person SET age = 30 WHERE age;",
        "why_a_bug": "SELECT's WHERE runs the 2005 not-bool check (data/select.rs check_where_clause), but mutation WHEREs go through analyze_expression_positions_for (data/mutation.rs) which only infers + checks value-expressions + field paths \u2014 it never applies definitely_not_bool. So a non-bool predicate on UPDATE/DELETE/UPSERT (truthiness-tested at runtime, almost always a mistake) escapes. Same fix already exists for SELECT; low-FP to mirror."
      },
      {
        "title": "Whole-table SELECT without WHERE (new perf lint, distinct from 7009)",
        "proposed_code_family": "7xxx (new)",
        "default_level": "allow",
        "fp_risk": "medium",
        "example_surql": "SELECT * FROM person;",
        "why_a_bug": "An unbounded full-table scan with no WHERE/LIMIT is a performance footgun the owner asked to surface, parallel to 7009's whole-table UPDATE/DELETE. Legitimately intended often enough (small tables, admin queries) that it must be opt-in \u2014 default allow. Trivially detectable: table-target FROM with no where_clause and no LIMIT."
      },
      {
        "title": "Bare `SELECT *` for perf (new lint, distinct from 7007's redundant case)",
        "proposed_code_family": "7xxx (new)",
        "default_level": "allow",
        "fp_risk": "high",
        "example_surql": "SELECT * FROM person;",
        "why_a_bug": "Owner-requested: `SELECT *` over-fetches every column when named projections would be cheaper and more stable against schema drift. Distinct from 7007 (which fires only on the redundant `SELECT *, age` overlap). This is a pure style/perf opinion many teams reject, so it must be opt-in \u2014 default allow."
      }
    ]
  },
  {
    "family": "3xxx-graph + 4xxx-statement",
    "summary": "Audited every code in the graph-traversal (3001, 3002, 3004, 3009, 3011) and statement-shape (4003, 4004, 4005, 4006, 4007, 4010, 4017, 4019, 4020, 4021, 4022, 4023) families at their real emit sites, plus six catalog-registered-but-unemitted codes (4001, 4002, 4009, 4012, 4013, 4016). MESSAGE QUALITY: most messages are already consequence-first with a useful help line and a definition note (3009, 3011, 4004, 4005, 4021, 4022, 4023 are exemplary). Three weak spots: (a) the second 4003 emit site in mutation.rs is terser than the SELECT one and has no help line; (b) 3002 and 4019 lead with the declaration rather than the consequence; (c) several deny-level codes (4006, 4007, 4010) have no help: line. COMPLETENESS: 4009 (LIVE SELECT unsupported clause) is reserved but genuinely unimplementable today because LiveSelectStmt lowering discards everything except the table (the catalog's \"\ud83d\udd36 lowering keeps clauses\" note is stale/wrong \u2014 it does NOT). No whole-table-SELECT lint and no generic SELECT-* perf lint exist. 4023 only catches bare count(); the same per-row trap applies to scalar-column math:: aggregates without GROUP. DEFAULT LEVEL: the graph/statement contract violations correctly stay deny (3001, 3002, 3009, 4003, 4004, 4005, 4007). Two deny-level codes are questionable and likely over-strict: 3004 (FROM ending on an edge is legal SurrealQL \u2014 it returns edge records \u2014 so this is advisory, not an error) and 4021 (SHOW CHANGES on a non-changefeed table reads empty rather than erroring, per the message's own \"reads nothing\" wording \u2014 a no-op, not a hard error). Both should likely be warn. Proposed new perf/style lints (whole-table SELECT, SELECT *) default to allow per product-owner guidance.",
    "codes": [
      {
        "code": "3001",
        "recommended_default_level": "deny",
        "level_rationale": "Traversing a non-relation table with ->/<- is a hard contract violation; the step cannot mean anything. Error is correct.",
        "current_message_ok": true
      },
      {
        "code": "3002",
        "recommended_default_level": "deny",
        "level_rationale": "Wrong-direction/wrong-endpoint traversal against a declared relation shape is a genuine contract violation; the declared-vs-written note makes it actionable. Error is correct.",
        "current_message_ok": false,
        "message_rewrite": "Lead with the consequence, not the declaration. Current: 'relation `likes` connects `person`->`likes`->`post`, but this step traverses `->` from `person`'. Better: 'this step can't traverse `likes` from `person` \u2014 the relation runs `person`->`likes`->`post`' with help: 'traverse from a table on the near side, or reverse the arrow'. Keep the existing `with_related` note pointing at DEFINE TABLE."
      },
      {
        "code": "3004",
        "recommended_default_level": "warn",
        "level_rationale": "OVER-STRICT AS DENY. `SELECT * FROM person->likes` (a FROM idiom ending on an edge) is valid SurrealQL \u2014 it returns the edge record links. This is not a runtime error, so Error is a false-positive-shaped hard failure. Downgrade to warn (advisory: 'you're selecting the edge records; did you mean to land on the target node?') or verify against a live SurrealDB before keeping it deny.",
        "current_message_ok": true
      },
      {
        "code": "3009",
        "recommended_default_level": "deny",
        "level_rationale": "A traversal step whose starting field holds no records can never step anywhere \u2014 a hard contract violation. Message is already consequence-first with a good help line. Error is correct.",
        "current_message_ok": true
      },
      {
        "code": "3011",
        "recommended_default_level": "warn",
        "level_rationale": "Unbounded graph recursion (`{..}` / `{1..}`) is legal but a runaway-cost hazard, not a contract violation. Advisory-on-by-default is right; message is exemplary (consequence + concrete `{1..5}` fix). Keep warn.",
        "current_message_ok": true
      },
      {
        "code": "4001",
        "recommended_default_level": "deny",
        "level_rationale": "Reserved, correctly not emitting: invalid clauses (`CREATE ... WHERE`) are rejected at the parser/lowering layer, so no analyzer emission is needed. Keep reserved at E.",
        "current_message_ok": true
      },
      {
        "code": "4002",
        "recommended_default_level": "deny",
        "level_rationale": "Reserved, correctly not emitting: `SELECT VALUE a, b` is rejected by both SurrealDB's parser and ours (parse-level, 0xxx). No analyzer work needed. Keep reserved at E.",
        "current_message_ok": true
      },
      {
        "code": "4003",
        "recommended_default_level": "deny",
        "level_rationale": "ONLY on a table-wide target is a deterministic runtime error (SingleOnlyOutput). Error is correct. But the two emit sites are inconsistent \u2014 the SELECT site (select.rs) is strong; the mutation site (mutation.rs) is weaker.",
        "current_message_ok": false,
        "message_rewrite": "The mutation.rs emit ('ONLY on a whole table needs a record id target') is terser than select.rs and has no help line. Align it consequence-first with the runtime fact: 'this ONLY target can match many rows, but ONLY must resolve to exactly one \u2014 SurrealDB errors here at runtime' with help: 'target a record id like `person:tobie`'. (The SELECT-site message and help are fine as-is.)"
      },
      {
        "code": "4004",
        "recommended_default_level": "deny",
        "level_rationale": "INSERT column/value count mismatch is an unambiguous structural error. Message states the exact counts and gives a fix. Error is correct.",
        "current_message_ok": true
      },
      {
        "code": "4005",
        "recommended_default_level": "deny",
        "level_rationale": "BREAK/CONTINUE outside any FOR loop is dead/meaningless control flow \u2014 a near-certain bug. Message is consequence-first with a fix. Error is correct.",
        "current_message_ok": true
      },
      {
        "code": "4006",
        "recommended_default_level": "warn",
        "level_rationale": "Unreachable-after-divergence is advisory (dead code, not a runtime failure) and carries the Unnecessary tag for fade-out rendering. Warn is correct. Minor: no help line.",
        "current_message_ok": false,
        "message_rewrite": "Message is clear ('this statement is unreachable \u2014 the block already returned') but add a one-line help for consistency with peers, e.g. help: 'remove it, or move it before the RETURN/BREAK/THROW'."
      },
      {
        "code": "4007",
        "recommended_default_level": "deny",
        "level_rationale": "Transaction pairing (unopened COMMIT/CANCEL, nested BEGIN, unclosed BEGIN) is a hard contract violation. Error is correct. Two of the three messages lack a help line.",
        "current_message_ok": false,
        "message_rewrite": "'COMMIT/CANCEL without an open BEGIN' and 'BEGIN inside an open transaction; transactions do not nest' are clear but help-less. Add fixes: for the unopened case help: 'remove it, or add a matching `BEGIN` above'; for the nested case help: 'close the first transaction with COMMIT/CANCEL before opening another'. The 'this BEGIN is never closed; add COMMIT or CANCEL' variant already inlines its fix and is fine."
      },
      {
        "code": "4009",
        "recommended_default_level": "deny",
        "level_rationale": "Contract-clear (GROUP/ORDER/LIMIT/SPLIT on LIVE SELECT are rejected by SurrealDB) so E is right IF implemented \u2014 but it currently CANNOT emit: LiveSelectStmt lowering keeps only the table and drops all clauses, so there is nothing to check. The catalog's '\ud83d\udd36 lowering keeps clauses' status note is inaccurate. See completeness gap.",
        "current_message_ok": true
      },
      {
        "code": "4010",
        "recommended_default_level": "warn",
        "level_rationale": "Duplicate SET target is legal (last write wins) but almost always a mistake \u2014 advisory. Warn is correct. Message states the consequence ('the last assignment wins').",
        "current_message_ok": true
      },
      {
        "code": "4012",
        "recommended_default_level": "warn",
        "level_rationale": "Reserved, correctly not emitting: the grammar only accepts OMIT alongside a `*` projection, so the misuse is parser-covered. Keep reserved at W.",
        "current_message_ok": true
      },
      {
        "code": "4013",
        "recommended_default_level": "warn",
        "level_rationale": "Reserved and unimplemented ('verify exact semantics first'). GROUP BY key not in projections is a plausible advisory lint but SurrealDB's aggregate/grouping semantics need pinning down before emitting to avoid false positives. Keep reserved at W pending verification.",
        "current_message_ok": true
      },
      {
        "code": "4016",
        "recommended_default_level": "allow",
        "level_rationale": "Reserved, correctly not emitting and effectively unreachable: `{}` in value position is an empty object literal (not a block) and statement-position blocks have no consumed value. If ever revived it is pure style, so allow (opt-in) would be right; today keep reserved at I/Hint.",
        "current_message_ok": true
      },
      {
        "code": "4017",
        "recommended_default_level": "warn",
        "level_rationale": "A value-position block ending in LET silently evaluates to NONE \u2014 a near-certain bug in a block used as a value. Warn is correct; message is consequence-first with an inline fix ('return the value instead').",
        "current_message_ok": true
      },
      {
        "code": "4019",
        "recommended_default_level": "warn",
        "level_rationale": "CREATE/INSERT on a relation table without in/out produces a dangling edge \u2014 legal but almost always wrong, and RELATE is the idiomatic form. Warn is correct.",
        "current_message_ok": false,
        "message_rewrite": "Leads with the fix rather than the consequence. Current: '`likes` is a relation; use RELATE (or provide `in` and `out`)'. Better: 'creating `likes` without `in`/`out` makes an edge that connects nothing' with help: 'use `RELATE a->likes->b`, or set both `in` and `out`'."
      },
      {
        "code": "4020",
        "recommended_default_level": "warn",
        "level_rationale": "RETURN BEFORE on CREATE always yields NONE (no before-state) \u2014 a meaningless clause, advisory. Warn is correct; message states the consequence and the reason.",
        "current_message_ok": true
      },
      {
        "code": "4021",
        "recommended_default_level": "warn",
        "level_rationale": "QUESTIONABLE AS DENY. The message itself says SHOW CHANGES 'reads nothing' \u2014 i.e. it returns an empty change feed, not a runtime error, when the table has no CHANGEFEED. A no-op that silently returns nothing is advisory, not a hard failure, so Error over-classifies it. Downgrade to warn (or verify SurrealDB actually errors before keeping deny). Message + help are otherwise good.",
        "current_message_ok": true
      },
      {
        "code": "4022",
        "recommended_default_level": "warn",
        "level_rationale": "SELECT from a DROP table can never return rows (DROP discards every row on write) \u2014 a near-certain logic bug, not a perf/style opinion, so warn (on by default) is right, not allow. Message is consequence-first with a good help line.",
        "current_message_ok": true
      },
      {
        "code": "4023",
        "recommended_default_level": "warn",
        "level_rationale": "count() without GROUP silently yields 1-per-row instead of a total \u2014 a silent-wrong-result bug that downstream guards depend on. Warn is right (arguably deny, but warn is safe). Message names the trap and the GROUP ALL fix. See completeness gap for extending beyond bare count().",
        "current_message_ok": true
      }
    ],
    "completeness_gaps": [
      {
        "title": "LIVE SELECT unsupported clauses (4009) cannot fire \u2014 lowering discards all clauses",
        "proposed_code_family": "4009",
        "example_surql": "LIVE SELECT * FROM person GROUP BY city;",
        "why_a_bug": "SurrealDB's LIVE SELECT accepts only projections/DIFF, WHERE, and FETCH; GROUP/ORDER/LIMIT/START/SPLIT are rejected. 4009 is reserved for exactly this, but LiveSelectStmt (crates/syntax/src/ast/statement.rs:498) stores only `table: Option<Spanned<String>>` and its lowering (crates/syntax/src/lower/statement.rs:104) keeps only the table \u2014 so the analyzer never sees the offending clauses and can never emit 4009. The catalog's status note '\ud83d\udd36 LiveSelect lowering keeps clauses' is inaccurate. Requires extending the LiveSelectStmt AST + lowering to retain the clause spans, then emitting 4009 for the disallowed ones.",
        "fp_risk": "low",
        "default_level": "deny"
      },
      {
        "title": "No whole-table SELECT-without-WHERE lint (SELECT analogue of 7009)",
        "proposed_code_family": "40xx (new; sibling of 7009)",
        "example_surql": "SELECT * FROM person;",
        "why_a_bug": "A whole-table scan with no WHERE/LIMIT is legal and sometimes intended, but is a common accidental full-table read in application code. 7009 already covers whole-table UPDATE/DELETE; the read side has no equivalent. Per product-owner guidance this is a perf rule people may legitimately not want, so it must be OPT-IN (allow), distinct from the contract codes. Emit at the SELECT statement-shape site (analyzer/data/select.rs check_select_statement_shape) when the FROM is a bare table with no where_clause and no record-id target.",
        "fp_risk": "low",
        "default_level": "allow"
      },
      {
        "title": "No generic `SELECT *` perf lint (distinct from 7007's redundant case)",
        "proposed_code_family": "70xx (new; distinct from 7007)",
        "example_surql": "SELECT * FROM person;",
        "why_a_bug": "7007 only flags a wildcard that is redundant alongside explicit fields (`SELECT *, age`). It does not flag a plain `SELECT *` as a perf/precision concern (over-fetching every column). Per product-owner guidance, a general 'avoid SELECT *' rule is a style/perf opinion many teams won't want, so it must be OPT-IN (allow) and separate from 7007. Emit when the sole projection is a Wildcard and the author opted the lint on.",
        "fp_risk": "low",
        "default_level": "allow"
      },
      {
        "title": "4023 (count-per-row trap) misses scalar-column math:: aggregates without GROUP",
        "proposed_code_family": "4023 (extend) or new sibling",
        "example_surql": "SELECT math::sum(amount) FROM invoice;",
        "why_a_bug": "Without GROUP, SurrealDB evaluates the aggregate per row, so `math::sum(amount)` returns one row per invoice each holding that row's own amount \u2014 never the total the author intended, exactly the trap 4023 catches for count(). is_bare_count() (analyzer/data/select.rs) only matches count/count::count. Note the analyzer already promotes a scalar column to array in column_aggregate_kind() to suppress the 5002 type error, which quietly masks this per-row-vs-total confusion. Extending is worthwhile but must stay narrow: fire only for a single bare *scalar* column argument (a column already `array` \u2014 `math::sum(scores)` \u2014 is a legitimate per-row reduction and must not be flagged).",
        "fp_risk": "medium",
        "default_level": "warn"
      }
    ]
  },
  {
    "family": "5xxx-functions + 6xxx-params",
    "summary": "Audited all 12 codes in the functions/params family against their real emit sites and SurrealDB semantics. Emitting today: 5001 (unknown fn), 5002 (arity+arg-kind, builtin & fn:: & closure), 5005 (type::field const path), 5009 (fn:: non-termination), 6001 (conflicting param constraints), 6003 (analyzer-limitation hint), 6004 (use-before-LET), 6005 (context param outside context), 6007 (protected-param assignment). Registered but NOT implemented: 5010 (event self-trigger, marked \ud83d\udd28), 6002 (param shadows DEFINE PARAM w/ different kind, \ud83d\udd36), 6006 (host type contradicts constraint, \ud83d\udd28 adapter). Default levels: this family is almost entirely contract violations (deny) or advisories (warn) \u2014 no perf/style opt-in rules belong here, EXCEPT 6003 which reports SurrealGuard's own limitation and reads as noise, so I recommend it move to allow (opt-in). Message quality is mostly good and consequence-first, but three real defects: (1) builtin 5001 lacks the did-you-mean help that the fn:: path already has; (2) the two builtin 5002 sites in signature.rs have no help/fix line while the fn:: 5002 site does \u2014 inconsistent; (3) 5005's non-string branch renders the value with Rust Debug (`{other:?}`) leaking internal enum variants like `Number(Int(42))`. Completeness gaps worth adding: type::thing/type::table const table-name validation, math::fixed invalid places constant, plus finishing 5010/6002/6006.",
    "codes": [
      {
        "code": "5001",
        "recommended_default_level": "deny",
        "level_rationale": "Calling a function that does not exist is a hard contract violation \u2014 SurrealDB errors at runtime. Error is correct.",
        "current_message_ok": false,
        "message_rewrite": "The fn:: emit site is good (it offers `did you mean \\`fn::x\\`?` via suggest::closest). The builtin fallthrough (unknown_function in function/mod.rs) is weaker: `\\`string::lenght\\` is not a known function` with NO help line. Give it parity: run suggest::closest over the known builtin names and add help `did you mean \\`string::length\\`?`, else `no builtin or DEFINE FUNCTION named \\`string::lenght\\` exists`. A bare typo in a builtin name is the single most common case and currently gets no fix hint."
      },
      {
        "code": "5002",
        "recommended_default_level": "deny",
        "level_rationale": "Arity and argument-kind mismatches are contract violations that fail at runtime. Error is correct.",
        "current_message_ok": false,
        "message_rewrite": "The fn:: arg-kind site (function/mod.rs) is exemplary: consequence + help `pass a \\`int\\`, or widen \\`$x\\` to accept \\`string\\`` + note `\\`fn::f\\` is defined here`. But the two BUILTIN sites in signature.rs have NO help/fix line. Add one for consistency: for arity `check_arity` emit help `\\`string::len\\` signature: string::len(string) -> int`; for `check_argument_kinds` emit help `pass a \\`string\\``. Also note the code's own doc-comments still reference retired numbers 5003/5004 (function/signature.rs:76 says 'findings 5002/5003'; function/mod.rs check_closure_arity comment says '(5004)') \u2014 stale, should read 5002."
      },
      {
        "code": "5005",
        "recommended_default_level": "deny",
        "level_rationale": "A const argument that provably violates the function's value contract (e.g. type::field naming no field) is a near-certain bug. Error is correct.",
        "current_message_ok": false,
        "message_rewrite": "field.rs:71 (`\\`ghost\\` is not a field of table \\`person\\``) is clear \u2014 optionally add a did-you-mean help over the table's field paths. The real defect is field.rs:30: `format!(\"type::field expects a field-path string, found \\`{other:?}\\`\")` uses Rust Debug on a surrealdb_types::Value, leaking internal variants (a user sees `Number(Int(42))`, not `42`). Rewrite consequence-first with Display: message `type::field needs a field-path string, but this argument is the {kind} \\`{value}\\`` and help `pass a string naming a field, e.g. type::field('name.first')`, rendering value via Display/render_kind, never {:?}."
      },
      {
        "code": "5009",
        "recommended_default_level": "warn",
        "level_rationale": "Unconditional recursion never terminating is a near-certain bug, but detection is a text-scan guardedness heuristic (fn_body_branches greps for if/for), so a warn \u2014 not a deny \u2014 respects the small false-positive surface. Keep Warning.",
        "current_message_ok": false,
        "message_rewrite": "Message `\\`fn::fac\\` never terminates: fn::fac -> fn::fac calls itself` is consequence-first but reads awkwardly for mutual cycles (`A -> B calls itself`) and offers no fix. Add help `add an IF/FOR base case so the recursion can stop, or break the cycle`. For a single-element cycle the joined path is just the name, so consider `... : it calls itself with no base case` vs mutual `... : the cycle A -> B -> A has no base case`."
      },
      {
        "code": "5010",
        "recommended_default_level": "warn",
        "level_rationale": "Event self-trigger / trigger cycles are advisory: a bounded cascade can be legitimate, and SurrealDB has depth limits, so this is a warn not an error. NOT YET IMPLEMENTED (catalog marks it \ud83d\udd28 event-effect graph) \u2014 registered only. Keep as Warning when built.",
        "current_message_ok": false,
        "message_rewrite": ""
      },
      {
        "code": "6001",
        "recommended_default_level": "deny",
        "level_rationale": "Irreconcilable constraints on one param mean no host value can satisfy the query \u2014 a genuine contradiction. Error is correct.",
        "current_message_ok": true,
        "message_rewrite": "Good: `\\`$x\\` cannot satisfy this query: one use needs \\`int\\`, this one needs \\`string\\`` is consequence-first with both kinds inline. Only enhancement: attach a note: span at the FIRST constraining use (it currently anchors only the second site), so the reader sees both positions."
      },
      {
        "code": "6002",
        "recommended_default_level": "warn",
        "level_rationale": "A LET that shadows a DEFINE PARAM with a different kind is advisory (the local binding legitimately wins) \u2014 Warning, not Error. NOT IMPLEMENTED: only referenced in config.rs policy test, never emitted. Keep Warning when built.",
        "current_message_ok": false,
        "message_rewrite": ""
      },
      {
        "code": "6003",
        "recommended_default_level": "allow",
        "level_rationale": "This reports SurrealGuard's OWN inability to analyze a type ('surrealguard can't analyze ... yet'), not a user contract violation. It is not actionable by most users and reads as tool noise. Move from Hint(on) to allow (opt-in) so only users who want to audit analyzer coverage see it. This is the one code in the family that fits the product-owner 'opt-in' bucket.",
        "current_message_ok": true,
        "message_rewrite": "Message + help (`unsupported type syntax: ...`) are honest and fine as-is; the change is the default level, not the text."
      },
      {
        "code": "6004",
        "recommended_default_level": "warn",
        "level_rationale": "Reading a param before its own LET runs (in the same source) yields NONE/stale \u2014 near-certain a bug, but warn is the conservative default since a host could also supply the name. Keep Warning.",
        "current_message_ok": false,
        "message_rewrite": "`\\`$x\\` is read before its LET on this line runs` states the ordering but not the consequence. Rewrite consequence-first: `\\`$x\\` is read here before its LET runs, so it is NONE` and add help `move the LET above this use` + note: at the LET span. The note anchor makes the ordering obvious in an editor."
      },
      {
        "code": "6005",
        "recommended_default_level": "deny",
        "level_rationale": "$before/$after/$value/$event/$input used outside the event or field-VALUE construct that binds them are genuinely unbound (NONE) \u2014 a real error. Error is correct.",
        "current_message_ok": false,
        "message_rewrite": "`\\`$before\\` only exists inside the construct that binds it` is vague about both the consequence and the valid context. Rewrite: `\\`$before\\` is not bound here, so it reads NONE` with help `\\`$before\\`/\\`$after\\`/\\`$value\\` exist only inside DEFINE EVENT ... THEN and field VALUE/ASSERT clauses`. Naming the binding contexts is the fix most users need."
      },
      {
        "code": "6006",
        "recommended_default_level": "deny",
        "level_rationale": "A host-declared param type that contradicts what the query requires is a static contradiction provable at the adapter boundary. Error is correct. NOT IMPLEMENTED (catalog marks it \ud83d\udd28 adapter layer; registered so it is not dropped). Keep Error when the TS/Rust adapter feeds host types in.",
        "current_message_ok": false,
        "message_rewrite": ""
      },
      {
        "code": "6007",
        "recommended_default_level": "deny",
        "level_rationale": "SurrealDB rejects LET assignment to protected/context params ($auth, $session, $this, ...) at runtime. Error is correct.",
        "current_message_ok": true,
        "message_rewrite": "Good: consequence-first message + help naming the protected set. No change needed. (Note the PROTECTED list here and CONTEXT_ONLY_PARAMS for 6005 overlap but differ \u2014 $auth/$session/$token/$access/$parent are protected-only; worth a comment so they don't drift.)"
      }
    ],
    "completeness_gaps": [
      {
        "title": "type::thing / type::table with a const string naming an unknown table (5005 family)",
        "example_surql": "type::thing('prson', 42)  -- 'prson' is not a defined table; type::table('ghost')",
        "why_a_bug": "The catalog explicitly lists this as a 5005 case (marked \ud83d\udd36). type::thing/type::table currently use ParamKind::Any and never inspect a const first argument, so a typo'd table name in a record/table constructor is silently accepted even though 1001 flags the identical mistake for a bare table reference. When the argument is a const string and the workspace is typed, it is the same contract as 1001.",
        "fp_risk": "medium",
        "proposed_code_family": "5005",
        "default_level": "deny",
        "why_a_bug_extra": "FP note: only fire on a statically-known string literal (const_value_arg) AND only in a typed/schemafull workspace \u2014 schemaless setups legitimately reference tables that are not DEFINEd, so a naive check would false-positive there."
      },
      {
        "title": "math::fixed (and similar) invalid constant place/precision argument (5005 family)",
        "example_surql": "math::fixed(3.14159, -1)  -- negative decimal places",
        "why_a_bug": "Catalog lists 'out-of-range constants (math::fixed(x, -1))' as an intended 5005 case (\ud83d\udd36). math::fixed today only declares ParamKind::Numeric for both args and never validates the const value of the second, so an out-of-range literal precision passes analysis.",
        "fp_risk": "low",
        "proposed_code_family": "5005",
        "default_level": "deny",
        "why_a_bug_extra": "FP note: restrict to literal negative/non-integer constants via const_value_arg so a runtime value never trips it."
      },
      {
        "title": "Finish 5010 event self-trigger / trigger-cycle detection",
        "example_surql": "DEFINE EVENT bump ON person WHEN true THEN (UPDATE person SET n = n + 1)",
        "why_a_bug": "An event whose THEN mutates the same table (directly, or A->B->A) can recurse until SurrealDB's depth limit aborts the transaction. Registered as 5010/Warning but unimplemented. It is the events analogue of 5009's fn:: cycle detection and reuses the same three-color DFS over an event-effect graph.",
        "fp_risk": "medium",
        "proposed_code_family": "5010",
        "default_level": "warn",
        "why_a_bug_extra": "FP note: bounded/guarded cascades are legitimate; mirror 5009 and only flag cycles with no branching guard. Warn, not deny."
      },
      {
        "title": "Finish 6002 param shadows a DEFINE PARAM with a different kind",
        "example_surql": "DEFINE PARAM $min_age VALUE 18; ... LET $min_age = 'x';",
        "why_a_bug": "A LET that rebinds a globally DEFINEd param to a different kind is almost always a mistake or a name collision. Registered 6002/Warning but only referenced in a policy test, never emitted \u2014 the DEFINE PARAM kind is known at schema time and the LET kind is inferred, so the check is cheap.",
        "fp_risk": "low",
        "proposed_code_family": "6002",
        "default_level": "warn",
        "why_a_bug_extra": "Advisory (the local binding legitimately shadows), so Warning is right."
      }
    ]
  },
  {
    "family": "7xxx-lints + 8xxx-version",
    "summary": "Audited all 14 codes in the lint + version-compat families against their real emit sites and SurrealDB core semantics. Message quality: the newest lints (7002 shadowing, 7007 wildcard, 7008 schemaless, 7009 whole-table, 7012 computed, 7013 suppression) already meet the consequence-first + help: voice target. Four codes still emit bare fact-first messages with no help: line and should be brought into line \u2014 7003 (mixed array), 7005 (always-false compare), 7006 (empty membership), 7011 (id in SET). Three catalog entries are UNIMPLEMENTED (no emit site anywhere): 7001 (unused LET), 8001 and 8003 (entire version-compat family \u2014 no version registry exists). Default-level: the current policy (policy.rs) forces every 7xxx Lint code to Warn regardless of catalog severity; per the product owner the perf/safety/style lints (7002, 7003, 7008, 7009) should default to ALLOW (opt-in) while provable-bug lints (7004/7005/7006) and contract-ish lints (7007/7011/7012/7013) stay Warn, and the Compat family (8001/8003) stays Deny (intrinsic Error). Completeness: 7012 misses DEFINE EVENT bodies (catalog promises \"field VALUE or event body\" but only DEFINE FIELD is walked); the whole version family is a stub; and two new opt-in perf lints are proposed (whole-table SELECT, any SELECT *).",
    "codes": [
      {
        "code": "7001",
        "current_message_ok": false,
        "recommended_default_level": "warn",
        "level_rationale": "Unused binding is the canonical warn-by-default lint (cf. rustc unused_variables); advisory but on by default. NOTE: currently has NO emit site \u2014 unimplemented despite the catalog note that env use-tracking exists.",
        "message_rewrite": "Unimplemented today. Suggested: message `$x` is bound here but never read \u2014 help: remove this LET, or reference `$x` later; note: span on the LET keyword."
      },
      {
        "code": "7002",
        "current_message_ok": true,
        "recommended_default_level": "allow",
        "level_rationale": "Shadowing is a legitimate, common pattern (clippy's shadow_* lints are all restriction/allow-by-default). The message is educational, not a bug report, so it should be opt-in. Message already leads with the consequence and carries the `silence with W7002` help.",
        "message_rewrite": ""
      },
      {
        "code": "7003",
        "current_message_ok": false,
        "recommended_default_level": "allow",
        "level_rationale": "Heterogeneous arrays are legal SurrealQL and sometimes intended (`array<int|string>`), so this is an opinion lint \u2192 opt-in. Message is fact-first (`array literal mixes kinds: int, string`) with no consequence and no help: line.",
        "message_rewrite": "message: this array mixes element kinds, so its type widens to `array<int | string>` \u2014 help: if that is intended, ignore this; otherwise make every element the same kind."
      },
      {
        "code": "7004",
        "current_message_ok": false,
        "recommended_default_level": "warn",
        "level_rationale": "A branch/loop decided by a constant is dead code \u2014 near-certain smell, keep visible by default. The empty-FOR variant is good (consequence + help); the IF variant lacks a help:/fix line and does not say which branch dies.",
        "message_rewrite": "IF variant: keep `this IF condition is constant, so one branch is never taken` but add help: the condition is always <true/false>; drop the IF and keep the live branch, or make the condition depend on a value."
      },
      {
        "code": "7005",
        "current_message_ok": false,
        "recommended_default_level": "warn",
        "level_rationale": "Value-proven always-false/true comparison (disjoint records, sentinel mismatch, literal-set exclusion) \u2014 high-confidence latent bug, low FP, keep Warn (not Deny: constant guards are occasionally deliberate). Message is consequence-first but has no help: line telling the user what to fix.",
        "message_rewrite": "keep `\"=\" between `X` and `Y` is always false` and add help: these kinds can never be equal, so this test is dead \u2014 check the operand or the expected literal set."
      },
      {
        "code": "7006",
        "current_message_ok": false,
        "recommended_default_level": "warn",
        "level_rationale": "Empty-collection or element-kind-disjoint membership is provably always false \u2014 dead filter, near-certain bug, low FP. Keep Warn. Both variant messages are clear but carry no help: line.",
        "message_rewrite": "add help to both variants: this membership test can never match \u2014 remove it, or check the collection/element you meant to test."
      },
      {
        "code": "7007",
        "current_message_ok": true,
        "recommended_default_level": "warn",
        "level_rationale": "The REDUNDANT case (`SELECT *, age`) \u2014 the explicit field is genuinely dead and signals a half-finished narrow; low FP, keep Warn. This is distinct from the proposed perf `SELECT *` lint (allow). Message + `remove the explicit field, or drop the *` help are good.",
        "message_rewrite": ""
      },
      {
        "code": "7008",
        "current_message_ok": true,
        "recommended_default_level": "allow",
        "level_rationale": "Product-owner mandate: schemaless-in-typed-workspace is a legitimate choice, so opt-in. Message states the consequence (field checks skipped) and gives a concrete fix (add DEFINE FIELD). Good as written.",
        "message_rewrite": ""
      },
      {
        "code": "7009",
        "current_message_ok": true,
        "recommended_default_level": "allow",
        "level_rationale": "Product-owner mandate: whole-table UPDATE/DELETE without WHERE is legal and sometimes intended (bulk migrations), so opt-in. Message leads with consequence (`writes every row`) and inlines the fix (`add WHERE or a record id`). Minor: only checks stmt.targets.first(), and does not cover UPSERT (correctly excluded \u2014 UPSERT generates a row).",
        "message_rewrite": ""
      },
      {
        "code": "7011",
        "current_message_ok": false,
        "recommended_default_level": "warn",
        "level_rationale": "Confirmed against core: `UPDATE person:x SET id = ...` throws IdMismatch (err/mod.rs:622 `Found ... for the id field, but a specific record has been specified`) \u2014 a real runtime failure, near-certain bug, keep Warn. Message states the rule but gives no help:/fix (e.g. set the id at CREATE time).",
        "message_rewrite": "keep `record ids are immutable; id is set at creation` and add help: choose the id when you create the record (`CREATE person:the_id ...`), not with SET on an existing row."
      },
      {
        "code": "7012",
        "current_message_ok": true,
        "recommended_default_level": "warn",
        "level_rationale": "Blocking/side-effecting call (http::*, sleep) in a computed context runs on every write \u2014 a strong footgun that is almost never intended; keep Warn (a team could reasonably move it to allow). Message leads with the consequence (`runs on every write to this row`) and has a clear help. See completeness gap: only DEFINE FIELD is walked, not DEFINE EVENT.",
        "message_rewrite": ""
      },
      {
        "code": "7013",
        "current_message_ok": true,
        "recommended_default_level": "warn",
        "level_rationale": "A malformed suppression directive (unknown code, name-not-code, missing required reason) silently fails to suppress anything \u2014 a real defect in the source the user must fix; keep Warn. All three messages already state the problem and inline a correct-form example.",
        "message_rewrite": ""
      },
      {
        "code": "8001",
        "current_message_ok": false,
        "recommended_default_level": "deny",
        "level_rationale": "A function absent from the configured target version means the query cannot run there \u2014 a hard contract violation \u2192 Deny (intrinsic Error; Compat family, not forced to Warn). UNIMPLEMENTED: no version registry and no emit site exist yet.",
        "message_rewrite": "Unimplemented. Suggested once the registry lands: message `array::fold` is not available in SurrealDB <target> \u2014 help: it was added in <version>; raise the target version or use <alternative>; for renames, suggest the new name (e.g. string::ends_with)."
      },
      {
        "code": "8003",
        "current_message_ok": false,
        "recommended_default_level": "deny",
        "level_rationale": "Syntax that a target version's parser rejects (closures, `??`) makes the query unparseable there \u2014 hard contract violation \u2192 Deny. UNIMPLEMENTED: shares the missing version registry; no emit site.",
        "message_rewrite": "Unimplemented. Suggested: message closure syntax `|$x| ...` requires SurrealDB <version>, but the target is <target> \u2014 help: raise the target version, or rewrite without the newer syntax."
      }
    ],
    "completeness_gaps": [
      {
        "title": "Version-compat family (8001/8003) is entirely unimplemented \u2014 no version registry, no emit sites",
        "proposed_code_family": "8xxx-version",
        "example_surql": "-- target = 1.x\nRETURN array::fold([1,2], 0, |$a, $b| $a + $b);",
        "why_a_bug": "The catalog registers 8001 (function unavailable in target version) and 8003 (syntax needs a newer version) as Errors, but nothing in the analyzer emits them \u2014 there is no version registry. Every version-incompatibility a user could hit is silently missed, so the whole family is a promised contract with no enforcement.",
        "fp_risk": "low",
        "default_level": "deny"
      },
      {
        "title": "7012 misses DEFINE EVENT bodies \u2014 only DEFINE FIELD VALUE/ASSERT/DEFAULT is walked",
        "proposed_code_family": "7012",
        "example_surql": "DEFINE EVENT notify ON TABLE order WHEN $event = 'CREATE' THEN {\n  http::post('https://hooks.example/notify', $after);\n};",
        "why_a_bug": "The catalog scopes 7012 to a computed context \u2014 'field VALUE or event body' \u2014 but check_computed_calls is only called from schema/define/field.rs. A DEFINE EVENT THEN block also runs on every matching write, so a blocking/side-effecting http::/sleep call there is the identical footgun and goes uncaught.",
        "fp_risk": "low",
        "default_level": "warn"
      },
      {
        "title": "NEW opt-in perf lint: whole-table SELECT with no WHERE and no LIMIT",
        "proposed_code_family": "7014 (new)",
        "example_surql": "SELECT * FROM person;",
        "why_a_bug": "A SELECT over a bare table target with neither WHERE nor LIMIT is an unbounded full-table scan \u2014 the read-side analogue of 7009. Not a contract violation (it is valid and sometimes intended in admin/analytics), so it must be opt-in, but it is a real performance footgun the product owner asked to surface as a lint.",
        "fp_risk": "medium",
        "default_level": "allow"
      },
      {
        "title": "NEW opt-in perf/robustness lint: any `SELECT *` (projects all columns), distinct from 7007's redundant case",
        "proposed_code_family": "7015 (new)",
        "example_surql": "SELECT * FROM person WHERE id = person:tobie;",
        "why_a_bug": "`SELECT *` over-fetches every field and makes result shapes brittle to schema changes; teams that want explicit projections cannot enforce it today. 7007 only fires on the redundant `SELECT *, field` overlap, never on a plain `SELECT *`. This is a pure opinion/perf rule, so opt-in with high FP tolerance \u2014 SELECT * is extremely common and often fine.",
        "fp_risk": "high",
        "default_level": "allow"
      },
      {
        "title": "7001 (unused LET) is unimplemented \u2014 no reader wired to emit it",
        "proposed_code_family": "7001",
        "example_surql": "LET $unused = 42;\nRETURN 1;",
        "why_a_bug": "The catalog registers 7001 (Warning) and notes env use-tracking exists, but no emit site references it anywhere. A LET whose binding is never read is dead code the tool claims to catch and does not. Watch FP on side-effecting RHS (`LET $_ = fn::audit()`), which argues for firing only when the RHS is pure.",
        "fp_risk": "medium",
        "default_level": "warn"
      },
      {
        "title": "7004 does not cover constant WHERE conditions \u2014 only IF and empty-FOR are emitted",
        "proposed_code_family": "7004",
        "example_surql": "SELECT * FROM person WHERE 1 = 1;",
        "why_a_bug": "The catalog lists `WHERE 1 = 1` as a 7004 example ('control flow decided by a constant'), but the emit sites are only if_else.rs (IF) and for_loop.rs (empty array). A constant-true/false WHERE is dead filtering and is never flagged. Lower priority: `WHERE true` is sometimes a deliberate query-builder placeholder, so guard for FP.",
        "fp_risk": "medium",
        "default_level": "warn"
      }
    ]
  }
]