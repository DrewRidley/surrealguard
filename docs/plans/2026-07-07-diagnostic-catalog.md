# Diagnostic catalog: categories, codes, and the parameter-constraint channel

Status: implemented (2026-07-10) — every code states the contract it
enforces, and every contract not marked reserved/parser-covered/pending
below is emitting from its analyzer. This is the canonical registry;
codes are assigned here and only here; retired numbers
are never reused.

## Principles (settled in prior rulings)

- **Contracts, not engine behavior.** Every construct has a contract — what
  the author must mean for the statement to make sense. A diagnostic exists
  when the contract is provably violated; whether SurrealDB throws, silently
  tolerates (kind-ordered comparisons), or coerces is *irrelevant to
  severity* — tolerated misuse is the reason this tool exists. One code per
  contract: variations of the same violation are message variants, never new
  codes (operand mismatch is 2004 whether binary, unary, arithmetic, or
  comparison; assignability is 2001 whether the value is a wrong kind or
  NONE).
- **Inference is never affected.** A finding never changes an inferred type.
  Most findings coincide with a perfectly known type (`UPDATE person SET
  age = 'x'` still types as person rows); poison (`Any`) appears only where
  the type is genuinely unknowable.
- **Detection site = emission site.** The analyzer that owns the statement
  emits its findings; composite analyzers learn about inner findings by
  span overlap when they need to.
- **Old validators are removed fully.** Fresh codes, fresh standardized
  messages; the old finding-tests are rewritten per site as it flips.
- **Statically provable only.** Every code below is checkable from source +
  schema. Anything requiring runtime values is out — with one deliberate
  exception: when a value is a *host parameter*, the check is exported as a
  constraint instead of emitted as a finding (see the last section).

## Code scheme

`E`/`W`/`I` prefix in rendering comes from severity, not the code. Codes are
`<family><3 digits>`, families numbered by category:

| Family | Range | Category |
|---|---|---|
| syntax | 0xxx | Parse-level breakage (already emitted by the parser) |
| schema | 1xxx | References to schema objects that don't exist |
| type | 2xxx | Kind mismatches and nullability |
| graph | 3xxx | Relation and traversal misuse |
| statement | 4xxx | Clause and statement misuse |
| function | 5xxx | Function and closure misuse |
| param | 6xxx | Parameter constraints and conflicts |
| lint | 7xxx | Style and suspicious-but-valid constructs |
| compat | 8xxx | SurrealDB version compatibility — retired and empty, see below |

Severity defaults: **error** = provably fails or misbehaves at runtime;
**warning** = provably suspicious but executable; **info** = analyzer
limitation or style note.

Severity is the finding's *intrinsic class*; it is data, not policy.
Consumers apply policy at their edge through `PolicyConfig`
(`warnings_as_errors` promotion, per-code allow/warn/deny for lints) —
the CLI for exit codes, the LSP for editor severities, host adapters for
build failures. Errors are never demotable; lints are fully configurable.
(Ruled 2026-07-07; implemented at the CLI and LSP edges.)

In-source suppression is different: a `-- surrealguard: allow(E1001)
reason="why"` comment is source-authored intent, so it applies at
analysis time, rustc-style — the directive covers the next line (or its
own line when trailing a statement) and a suppressed finding never
leaves the pipeline. Directives that violate their own contract are 7013.

Detection status legend: ✅ inference already computes everything needed
(emission is additive); 🔶 partial (needs modest new analysis at the site);
🔨 needs new analysis machinery (named).

---

## 1xxx — Schema references

Family contract: **every name a query uses must resolve against the schema,
wherever the schema constrains it.** One code per kind of name; the position
(projection, WHERE, SET, a DEFINE, a RELATE endpoint, a PATCH path) is
carried by the span and message, never by the code.

| Code | Contract | Covers (message variants) | Sev | Status |
|---|---|---|---|---|
| 1001 | a table reference names a known table | FROM/targets, thin statements, RELATE endpoints, `record<t>` in DEFINE FIELD, relation IN/OUT tables, DEFINE ... ON table | E | ✅ emitting (endpoints/DDL variants pending) |
| 1002 | a field reference names a declared field of its row's table (schemafull only; FLEXIBLE subtrees exempt) | projections, WHERE/expressions, SET/UNSET targets, payload keys, RETURN, OMIT, FETCH, SPLIT, GROUP/ORDER keys, INSERT columns, index/event fields in DEFINE, const PATCH paths | E | ✅ emitting (currently split across 1002-1011; renumbering to 1002) |
| 1012 | a schema-object reference names a known object of that kind | REBUILD/REMOVE INDEX, REMOVE EVENT, SEARCH ANALYZER refs, WITH INDEX hints | E | ✅ index/event registries in extraction (WITH hints pending grammar support) |
| 1021 | REMOVE removes something that exists | `REMOVE TABLE ghost` | W | ✅ (exists today) |
| 1022 | a definition does not silently redefine (OVERWRITE states intent) | two `DEFINE TABLE person` | W | ✅ |
| 1023 | FETCH names something that can hold records | `FETCH age` (int); an alias of a computed non-record value | E | ✅ emitting |
| 1024 | SPLIT names a collection field | `SPLIT age` | E | ✅ emitting |
| 1025 | a subfield is declared under an object-shaped parent | `FIELD a TYPE int` then `FIELD a.b` | E | 🔶 |
| 1027 | an index-backed operator has its supporting index | `@@`/search::* need a SEARCH index; `<\|k\|>` needs MTREE/HNSW | E | ✅ emitting |
| 1029 | each index covers a distinct field set | two indexes on `(email)` | W | ✅ emitting |
| 1032 | DEFINE ANALYZER components name known tokenizers/filters/languages | `FILTERS snowball(klingon)` — tokenizer names are parser-covered (the grammar hard-codes them); filter names/languages emit here | E | ✅ emitting |
| 1033 | a DEFINE FIELD clause is one the field it targets accepts | `id` rejects VALUE / READONLY / COMPUTED / DEFAULT ALWAYS — the engine fails the definition ("Cannot use the `VALUE` keyword on the `id` field"). A plain DEFAULT, TYPE, ASSERT, PERMISSIONS and COMMENT are all accepted on `id`; `in`/`out` were probed against 3.2.3 and have no clause restriction at all | E | ✅ emitting |

Folded by the contract audit (2026-07-09): 1003–1011 → 1002; 1013, 1014,
1030 → 1012; 1015 → 5001 (function resolution); 1016, 1017, 1018 → 1001;
1019, 1020, 1031 → 1002; 1026 → 1001 (source-order effects already make a
removed table unknown); 1028 → 1027.

## 2xxx — Types and nullability

| Code | Contract | Covers (message variants) | Sev | Status |
|---|---|---|---|---|
| 2001 | a value written to a field inhabits the field's declared type | SET values, CONTENT/MERGE/REPLACE payload values, INSERT tuple values and object payloads, DEFAULT and VALUE clauses in DEFINE, record-link targets (`record<a>` ⊄ `record<b>`), NONE into non-optional (message points at option<>) | E | ✅ emitting (SET/payload/tuple); DEFINE clauses 🔶 |
| 2004 | the operands make sense together for the operator | binary and unary, arithmetic and comparison, compound assignment (`age += 'x'`); SurrealDB kind-ordering instead of throwing changes nothing | E | ✅ emitting |
| 2005 | a condition position expects a boolean | IF conditions, ASSERT clauses, bare non-boolean WHERE | W | ✅ emitting (IF); ASSERT/WHERE 🔶 |
| 2007 | a cast names a known type | `<ghost> x` | E | ✅ emitting (allowlist of engine kind names) |
| 2008 | a conversion can succeed | kind-proven (`<duration> true`) or value-proven (`<int> 'abc'`, `type::int('x')`) — the proof strength varies, the contract doesn't | E | ✅ emitting |
| 2012 | a body returns what it declares | `fn::` `-> string { RETURN 1 }`; closures `\|$x\| -> string { RETURN 1 }` | E | ✅ both halves: fn:: bodies analyzed with params bound; closures |
| 2015 | a value-requiring position gets a value that is always present | `option<int>` field in `x + 1` | W | 🔶 needs the operand rule |
| 2017 | ORDER BY keys name fields available on the result rows (or RAND()) | non-field key; explicit projections not containing the key | E | ✅ emitting |
| 2018 | LIMIT/START take a non-negative integer | wrong kind (via params; literals parse-rejected), negative constants | E | ✅ emitting |
| 2019 | TIMEOUT takes a duration | parser-covered today; emission exists for when params are grammatical | E | ☑ parser-covered |
| 2020 | KILL takes a live-query uuid | parser-covered; grammar-fork bug: `KILL $id` fails to parse | E | ☑ parser-covered |
| 2021 | SHOW SINCE takes a versionstamp or datetime | | E | ✅ emitting |
| 2022 | FOR iterates something iterable | `FOR $x IN 42` — ranges must be modeled first or this false-positives | E | ✅ definite-scalar params; constant-empty iterables → 7004 |
| 2025 | READONLY fields are written only at creation | `UPDATE t SET created = ...` | E | ✅ emitting |
| 2026 | computed (VALUE-clause) fields are not hand-assigned | the write is silently overwritten | W | ✅ emitting |
| 2030 | index/filter/splat apply to collections | `age[0]`, `name[WHERE ..]`, `age.*` | E | ✅ emitting |
| 2031 | a regex literal compiles | `name ~ 'unclosed('` | E | ✅ emitting |
| 2032 | literal content is valid for its kind | `d'2024-13-45'`, `u'not-a-uuid'` | E | ✅ emitting |
| 2033 | PATCH operations are well-formed | unknown op, path without `/` | E | ✅ emitting |
| 2034 | required fields are provided at creation | `CREATE person;` with non-optional, no-DEFAULT `name` | E | ✅ emitting |
| 2035 | DEFINE ANALYZER filter arguments are valid | `edgengram(5, 2)` | E | ✅ emitting |
| 2036 | GeoJSON literals have their declared shape | `{type: 'Pointt', ...}` | E | ✅ emitting |
| 2037 | a field's DEFAULT satisfies its own ASSERT | `DEFAULT 'activ' ASSERT $value IN ['active','inactive']` | E | ✅ emitting |

Folded by the contract audit (2026-07-09): 2002, 2003, 2009, 2010, 2027 →
2001; 2006, 2011 → 2005; 2013 → 2012; 2014, 2029 → 2004; 2023 → 2008;
2024 → 2018.

## 3xxx — Graph and relations

Family contract: **a traversal or RELATE must use relations as declared.**

| Code | Contract | Covers (message variants) | Sev | Status |
|---|---|---|---|---|
| 3001 | a step traverses a relation table | `->person->` in a chain; a RELATE edge that is a plain table | E | ✅ emitting |
| 3002 | the usage matches the relation's declared shape (`in`->edge->`out`) | wrong-direction traversal, a hop landing off the far side, RELATE writing endpoints on the wrong sides — messages show declared vs written shape | E | ✅ emitting (as 3002/3003/3006; renumbering to 3002) |
| 3004 | a FROM-position chain is complete (edge->target pairs) | `FROM user->writes` | E | ✅ |
| 3009 | a traversal starts from records | `age->writes->` | E | ✅ |
| 3011 | graph recursion is bounded | `@{..}` with no upper bound | W | ✅ emitting |

Folded by the contract audit (2026-07-09): 3003, 3006 → 3002; 3007 → 3001;
3008 → 1001 (an endpoint naming an unknown table is a table-reference
violation); 3010 → 1023 (FETCH's contract). Deleted: 3005 — `->(a, b)` is
*valid*; not resolving it to one table is an analyzer limitation, not a
contract violation.

## 4xxx — Statement and clause misuse

| Code | Finding | Example | Sev | Status |
|---|---|---|---|---|
| 4001 | clause not valid on this statement | `SELECT ... RETURN NONE`, `CREATE ... WHERE` | E | ✅ lowering already isolates them |
| 4002 | SELECT VALUE with multiple projections | `SELECT VALUE a, b FROM t` — verified: both SurrealDB's parser *and* ours reject the syntax, so this is parse-level (0xxx); the code stays reserved, no analyzer emission | E | ☑ parser-covered |
| 4003 | ONLY on a table-wide target without LIMIT 1 | `SELECT * FROM ONLY person`, `UPDATE ONLY person` — deterministic runtime error (`SingleOnlyOutput`); CREATE is exempt (always one row) | E | ✅ |
| 4004 | INSERT tuple column/value count mismatch | `(a, b) VALUES (1)` | E | ✅ lowering counts |
| 4005 | BREAK/CONTINUE outside a loop | top-level `BREAK` | E | ✅ loop depth on ctx |
| 4006 | unreachable statements after RETURN/BREAK/THROW | `RETURN 1; SELECT ...` in a block | W | ✅ |
| 4007 | transaction pairing contract: BEGIN opens exactly one transaction that COMMIT/CANCEL closes | unopened COMMIT/CANCEL, nested BEGIN, BEGIN never closed | E | ✅ pipeline tracks the open transaction (nested/unpaired/unclosed variants) |
| 4009 | LIVE SELECT with unsupported clause | `LIVE SELECT ... GROUP BY` | E | ☑ parser-covered — the grammar admits only projections/FROM/WHERE/FETCH on LIVE SELECT, so GROUP/ORDER/LIMIT/etc. are a parse error, never a lowered clause |
| 4010 | duplicate SET target in one statement | `SET age = 1, age = 2` | W | ✅ assignments are structured |
| 4011 | duplicate projection key/alias | `SELECT age, age FROM t`, two `AS x` | W | ✅ keys computed |
| 4012 | OMIT without a wildcard projection | verified parser-covered: the grammar only accepts OMIT alongside `*` — code reserved, no emission | W | ☑ parser-covered |
| 4013 | GROUP BY field not in projections | SurrealDB aggregate rules | W | ✅ a group key absent from the projection can't label its rows (SurrealDB runs it → warning); `SELECT VALUE`/`*` exempt |
| 4016 | empty block | verified unreachable: `{}` in value position is an empty *object* literal, and statement-position blocks don't have their value consumed — code reserved, no emission | I | ☑ unreachable |
| 4017 | block ends with LET — its value is NONE | `{ LET $x = f(); }` consumed as a value | W | ✅ block value known |
| 4018 | side-effecting subquery in read position | `SELECT (CREATE log) FROM t` | W | ✅ statement kinds known |
| 4019 | CREATE/INSERT on a relation table without `in`/`out` | `CREATE likes SET strength = 1` | W | ✅ relation-ness known |
| 4020 | RETURN mode meaningless for the statement | `CREATE ... RETURN BEFORE` (always NONE) | W | ✅ (verify DELETE/AFTER semantics first) |
| 4021 | SHOW CHANGES on a table without CHANGEFEED | | E | ✅ |
| 4022 | SELECT from a DROP table | rows are never retained | W | ✅ |
| 4023 | count() without GROUP BY yields 1 per row, not a total | add GROUP ALL for a total | W | ✅ |
| 4024 | an IF branch is unreachable — its guard provably folds to a constant | `IF false { ... }`, the ELSE after `IF true { ... }` | W | ✅ constant-folded guard |
| 4025 | a wildcard projection cannot be aggregated by a GROUP clause | `SELECT * FROM t GROUP BY k`, `SELECT * FROM t GROUP ALL`, `SELECT *, count() FROM t GROUP BY k` — 3.0.5 rejects all of them outright (`Incorrect selector for aggregate selection, expression \`*\` … cannot be aggregated in a group`); 2.x silently drops the `*`, so the query never returns what its author asked for under either engine | E | ✅ verified on a live 3.0.5 |
| 4027 | a live query clause the notification will not reflect | `LIVE SELECT DIFF FROM t FETCH owner` — registers, delivers, and leaves the link a record id: the FETCH is silently dead. Also `LIVE SELECT name, DIFF FROM t`, where DIFF has lost its leading position and reads as an ordinary field path, so every notification carries `DIFF: null`. Sibling of 4009, which owns what the engine *refuses*; this owns what it accepts and then does not honour, hence W rather than E | W | ✅ verified on a live 3.2.3 — raw ws:// notification frames, per action (CREATE/UPDATE/DELETE) |
| 4026 | a filtered ONLY has no provable single-row target | `SELECT * FROM ONLY t WHERE status = 'open'` — errors (`Expected a single result output when using the ONLY keyword`) the moment two rows match, but succeeds while one does; W, not E, because the filter may well be single-row for reasons the schema does not state. Silent when at most one row is provable: a record-id target, `WHERE id = …`, an equality covering every field of a `UNIQUE` index, or `LIMIT 1`. Sibling of 4003, which owns the *unfiltered* table-wide case | W | ✅ verified on a live 3.0.5 |

Folded by the contract audit (2026-07-09): 4008, 4015 → 4007. Deleted:
4014 — no statable contract (RETURN is legal at top level and in blocks).

## 5xxx — Functions and closures

| Code | Contract | Covers (message variants) | Sev | Status |
|---|---|---|---|---|
| 5001 | a call resolves to a function that exists | unknown builtins, undefined `fn::`, methods not available on the receiver's kind | E | ✅ emitting (fn:: renumbering from 1015; methods pending) |
| 5002 | a call matches the function's signature | argument count, per-argument kinds (anchored per argument), `fn::` declared params, a closure declaring more parameters than its consumer binds | E | ✅ emitting (as 5002/5003/5004/5006; renumbering to 5002) |
| 5005 | a const argument satisfies the function's value contract | `type::field('aeg')` naming no field, non-string paths (`type::field(42)`), `type::thing('ghost', ..)` naming no table, out-of-range constants (`math::fixed(x, -1)`) | E | ✅ emitting (path cases); table/range variants 🔶 |
| 5009 | `fn::` definitions terminate (no direct/mutual recursion cycles) | `fn::f` calls `fn::f` | W | ✅ three-color DFS over hoisted signatures |
| 5010 | events do not trigger themselves (directly or in a cycle) | event on `person` THEN mutates `person`; A→B→A | W | 🔨 event-effect graph |

Folded by the contract audit (2026-07-09): 5003, 5004, 5006 → 5002; 5007,
5011, 5012 → 5005; 5008 → 5001; 1015 → 5001.

## 6xxx — Parameters

| Code | Finding | Example | Sev | Status |
|---|---|---|---|---|
| 6001 | conflicting constraints on one param | `WHERE $x > 3 AND $x = 'abc'` | E | ✅ unify at constraint sites; conflict emits at the second site |
| 6002 | param shadows a DEFINE PARAM with a different kind | `LET $min_age = 'x'` vs defined int | W | ✅ fires when the LET value's kind and the DEFINE PARAM's VALUE kind are both known and neither is assignable to the other |
| 6003 | unresolvable dynamic construct (analyzer limitation) | current `dynamic(6001)` class | I | ✅ |
| 6004 | param used before its LET in source order | `RETURN $x; LET $x = 1;` | W | ✅ env is source-ordered |
| 6005 | context param used outside its context | `$before` outside an event, `$parent` outside a subquery | E | 🔶 context-param model below |
| 6006 | host-declared type contradicts query constraint | host binds `$age: string`, query needs int | E (host-static) | 🔨 adapter layer; registered here so it is never dropped |
| 6007 | assignment to a protected parameter | `LET $auth = {...}` | E | ✅ protected-name list ($auth, $session, $token, $this, ...) |

## 7xxx — Lints

| Code | Finding | Example | Sev | Status |
|---|---|---|---|---|
| 7001 | unused LET binding | `LET $x = 1;` never read | W | ✅ textual reference scan over the rest of scope; opt-in (default `allow`) |
| 7002 | LET shadowing | inner `LET $x` over outer | I | ✅ scopes exist |
| 7003 | mixed-kind array literal | `[1, 'a']` | I | ✅ (today's partial fact) |
| 7004 | control flow is decided by a constant | `IF true`, `WHERE 1 = 1`, `FOR $x IN []` | W | ✅ emitting (IF); others 🔶 |
| 7005 | a comparison against a closed literal set must be able to match | `WHEN $event = 'CRATE'` — `$event` is `'CREATE' \| 'UPDATE' \| 'DELETE'`; value-proven always-false (distinct from 2004: the kinds are comparable) | W | ✅ emitting |
| 7006 | empty IN/CONTAINS list | `WHERE x IN []` | W | ✅ const arrays |
| 7007 | SELECT * with explicit fields | `SELECT *, age FROM t` | I | ✅ |
| 7008 | schemaless table in a typed workspace | queries against fieldless tables | I | ✅ |
| 7009 | whole-table UPDATE/DELETE without WHERE | `DELETE person;` | W | ✅ (deliberate ones silence per-code) |
| 7011 | assignment to `id` in SET | `SET id = ...` | W | ✅ |
| 7012 | blocking or side-effecting call in a computed context | `http::get(...)` / `sleep()` in a field `VALUE` or event body | W | ✅ call paths known |
| 7013 | a suppression directive names a catalog code (with a reason when required) | `-- surrealguard: allow(ghost)`; missing reason under `require_suppression_reasons` | W | ✅ |
| 7014 | whole-table SELECT with no WHERE and no LIMIT | `SELECT * FROM person;` | I | ✅ opt-in (allow by default) |
| 7015 | any bare `SELECT *` (over-fetch / schema-drift brittleness) | `SELECT * FROM person WHERE id = person:tobie;` | I | ✅ opt-in (allow by default) |

## 8xxx — Version compatibility (retired, empty)

**Retired 2026-08-08.** 8001 and 8003 were the only two codes here, both
gated on `[analysis] surrealdb_version` — a key `894f039` deleted because it
named a version and gated nothing. Neither code ever had an emission site,
so the family documented two contracts the analyzer could not enforce and
the generated diagnostics page advertised them to users as if it could.

Neither survives the config key's removal, because both contracts were
*about* having more than one target:

- **8001** — "every function used exists in the configured target version".
  There is no configured target: SurrealGuard analyzes for the latest
  release. A function that does not exist in that one release is already
  **5001**'s contract ("a call resolves to a function that exists"), which
  is where the retired spellings report. Keeping 8001 would be a second code
  for a single contract.
- **8003** — "syntax requires a newer version". With one target, syntax
  either parses or it does not, and not-parsing is a 0xxx parse-level code.
  There is no version axis left for this to measure against.

Both numbers are retired permanently and never reused, like the folded rows
below. Should per-version targeting ever return, it needs new numbers and a
real registry behind them — not two placeholders held open for it.

**After the contract audit (2026-07-09): ~80 contracts across 8 families**
(from 135 rows). Every row states its contract; message variants never get
their own codes; checks that cannot be phrased as contract violations were
deleted. Folded numbers are retired permanently — never reused.

---

## Context parameters

SurrealQL injects parameters by context; treating them as unbound would
both false-positive and forfeit real typing. The analyzer models them as
implicitly bound, with kinds where the context defines them:

| Param | Context | Kind |
|---|---|---|
| `$this` | any row scope | current row (table's object type) |
| `$parent` | subquery | enclosing row |
| `$event` | DEFINE EVENT | `'CREATE' \| 'UPDATE' \| 'DELETE'` (literal union) |
| `$before`, `$after` | DEFINE EVENT | the table's row type (`$before` optional on CREATE) |
| `$value` | DEFINE EVENT / FIELD `VALUE`/`ASSERT` | event row / the field's declared type |
| `$input` | DEFINE FIELD | the incoming (pre-coercion) value |
| `$auth`, `$session`, `$token`, `$scope` | permissions / anywhere | session-shaped objects (open) |

Consequences: the unbound-param collector excludes these names; using one
*outside* its context is finding **6005**; inside events, `$before.age`
resolves against the table like any field path. This is also what makes
`DEFINE EVENT`/`ASSERT` bodies fully type-checkable — and it composes:
`$event` being the literal union `'CREATE' | 'UPDATE' | 'DELETE'` means
`WHEN $event = 'CRATE'` (typo) is caught by 7005 (comparison always false)
with no event-specific code needed.

## Inference changes this catalog requires

Small inference upgrades several codes depend on (inference-first rule
still applies — these land before their findings):

- `IF` used as a value without `ELSE` includes `NONE` in its union
  (`LET $x = IF c { 1 }` is `int \| none`) — feeds 2015.
- `Recurse` idiom parts lower structurally (bounds retained) — feeds 3011.
- `ReadonlyClause`/`ValueClause`/`DefaultClause`/`AssertClause` lower on
  DEFINE FIELD — feeds 2009/2010/2011/2025/2026.
- INSERT `ON DUPLICATE KEY UPDATE` lowers into assignments — extends
  1004/2001 coverage.
- Index definitions carry their kind (search/vector/unique) in the schema
  index — feeds 1027/1028/1029 and 4013.

**False-positive prevention (correctness of existing rules, found by
checking the operand tables against real SurrealQL):**

- **Temporal arithmetic**: `datetime + duration → datetime`,
  `datetime - datetime → duration`, `duration ± duration → duration`,
  `duration * number → duration` are all valid — the current numeric-only
  arithmetic rule would make 2004 flag them. Both the inference result
  table and `binary_operands_compatible` gain these rows *before* 2004
  ships.
- **Collection concatenation**: `[1] + [2]` is array concat; `+` on two
  objects may merge (research). Same double-table treatment.
- **`<future>` casts**: `<future> { ... }` is not an unknown type — 2007
  would false-positive. Futures type as their inner expression's kind
  (lazily evaluated); needs grammar/lowering support.
- **Record-target assignability**: `record<a>` is assignable to
  `record<a | b>` but not to `record<b>` — `kind_is_assignable_to` gains
  target-set rules before 2001/2027 ship.
- **Compound assignment semantics**: `+=` means push on arrays, add on
  numbers, concat on strings — the 2029 table is also the inference rule
  for the mutated field's continuity.
- **Mock syntax** `|person:1000|` (batch create): grammar support unknown —
  verify; must not lower to a false Partial.
- **Ranges**: `1..5` is a first-class value (`Kind::Range`); `FOR $x IN
  1..5` is valid iteration — 2022 must accept ranges or it false-positives
  on the most common loop form. Range literals need lowering + inference.
- **Membership/geometry operators**: `CONTAINS`/`INSIDE`/`ALLINSIDE`/
  `INTERSECTS` have their own operand rules (string CONTAINS string is
  valid; geometry INTERSECTS geometry) — the 2004 table needs them before
  it ships.
- **Table views** (`DEFINE TABLE x AS SELECT ...`): the view's row type
  derives from its projection — an inference feature; every SELECT check
  applies inside the view definition.
- **Literal text retention**: datetime/duration/uuid literals keep their
  slice so 2032 can validate content and const tracking can carry their
  values.
- **Required-field metadata**: extraction records optionality and DEFAULT
  presence per field — feeds 2034.
- **FLEXIBLE fields**: a `DEFINE FIELD ... FLEXIBLE` object accepts
  arbitrary nested keys — every unknown-subfield check (1004/1005 and
  friends) must exempt paths under flexible fields or it false-positives
  on their intended use.
- **SCHEMAFULL gating**: the unknown-field family fires only on
  SCHEMAFULL tables (or declared fields of schemaless ones) — schemaless
  tables have open rows by design.
- **RELATE fan-out**: `RELATE [a:1, b:1]->likes->c:1` relates arrays of
  endpoints — 3006/3008 must accept record arrays, not just single ids.
- **Geometry subtypes**: `geometry<point>` vs `geometry<polygon>` — the
  assignability rules gain subtype awareness (upstream `Kind::Geometry`
  carries them) before 2001 touches geometry fields.

## Constraint discharge tiers

Every check is discharged somewhere; none are silently dropped. Three
tiers, decided per check by what is knowable where:

1. **Static** — provable from source + schema alone: emitted as findings
   here. (Most of the catalog.)
2. **Host-static** — needs the host's type information, provable at the
   *host's* compile time: exported as constraints; typed adapters (Rust
   macro, TS codegen) fail the build on violation. Example: `$age: int`
   against a host variable typed `string` (6006).
3. **Host-runtime** — the host language cannot prove it statically
   (dynamic JS, values from user input): the adapter emits a guard that
   validates before the query is sent. Example: `type::field($f)` with
   `$f` from a request body — the exported value domain
   (`{"name.first", "name.last", ...}`) becomes a runtime allowlist.

The export format carries enough for all three: kind, value domain,
origin spans (for host-side error messages that point back into the
query), and the discharge tier the analyzer could not achieve.

Future host-combined checks (design placeholders, no codes yet): table
permissions vs host-declared auth scope; record-id shape vs host id
types.

**Access payloads**: `DEFINE ACCESS ... SIGNUP (CREATE user SET email =
$email, pass = crypto::argon2::generate($pass))` defines, implicitly, the
typed signup payload — `$email: string, $pass: string` by the same
constraint collection. Exporting these gives host adapters fully typed
`signup()`/`signin()` calls for free.

## Research before coding (verify against SurrealDB, not docs-from-memory)

- GROUP BY projection rules (what exactly is legal ungrouped) → 4013.
- DDL inside transactions: allowed/atomic? → possible new 4xxx.
- Use-before-DEFINE in one script: runtime order vs our whole-workspace
  extraction → affects 1001/1026 precision.
- LIVE SELECT's exact clause restrictions → 4009.
- Event cascade semantics (depth limits?) → 5010 severity.
- `+` semantics on arrays/objects (concat/merge?) → temporal/collection
  operand tables.
- `<future>` evaluation semantics and grammar support → futures typing.
- INSERT/CREATE on relation tables: hard error or allowed? → 4019 severity.
- RETURN BEFORE/AFTER exact semantics per statement kind → 4020.
- THROW/RETURN inside transactions (early COMMIT? auto-CANCEL?) → 4xxx.
- ENFORCED relations (3.x): what becomes statically checkable → 3xxx.
- PATCH op validation timing (parse vs runtime) → 2033.

**Deferred design item — multi-database workspaces**: `USE NS/DB`
switches the schema everything after it resolves against. The current
`SchemaIndex` models one database; modeling `USE` means schema scoping
per (ns, db) with source-order switching. Out of scope for diagnostics
v1; queries after a `USE` targeting an unmodeled database degrade to
schemaless behavior (no false positives).

## The parameter-constraint channel (host adapters)

`UPDATE user SET age = $age` must not warn — it must *export*. Every use of
an unbound `$param` in a checkable position produces a **constraint** on
that parameter instead of a finding:

| Usage | Constraint produced |
|---|---|
| `SET age = $age` | `$age: int` (the field's kind) |
| `WHERE age > $min` | `$min: numeric` |
| `string::len($s)` | `$s: string` |
| `fn::greet($n)` | `$n: string` (from the DEFINE) |
| `type::field($f)` | `$f: string` **and** value ∈ table's field paths |
| `FROM $tbl` | `$tbl: table \| record` (domain: known tables) |
| `KILL $id` | `$id: uuid` |

As implemented, the constraint set rides on the exported `ParamInference`
(`crates/workspace/src/analysis.rs`) rather than a separate struct:

```rust
pub struct ParamInference {
    pub name: String,
    pub kind: Option<Kind>,               // unified across constraint sites
    pub domain: Option<ValueDomain>,      // beyond the kind, when known
    pub required: bool,                   // no DEFINE PARAM default
    pub spans: Vec<SourceSpan>,           // every use site
}

pub enum ValueDomain {
    /// Enumerable values (field paths for `type::field`, table names).
    OneOf(Vec<Value>),
    /// Numeric range (`LIMIT $n` → int, `0..`).
    Range { min: Option<i64>, max: Option<i64> },
}
```

Domains compose with kinds: `LIMIT $n` constrains `$n: int` *and*
`n >= 0`; a `LET $n = -1` elsewhere in the script makes the conflict a
static 6001. Hosts discharge domains at their tier — a TS adapter can
narrow `$f` to a string-literal union type, a runtime guard checks the
range.

- Constraints from multiple uses **unify** (intersection); an empty
  intersection is finding **6001** — the query cannot be satisfied by any
  value.
- `AnalysisOutput` carries constraints per source/statement; host adapters
  (Rust macro, TypeScript codegen) enforce them at the call site — `$age`
  must be a number at *the host's* compile time, and `$f` outside
  `{"name.first", "name.last", ...}` fails there too.
- The old kind-only inference was not replaced but extended: constraint
  sites feed `ParamInference.kind`/`.domain` directly, and plain uses
  still record name + span so unconstrained params export too.

## Rollout order

1. Registry plumbing: category enum ↔ `FindingCode` families, message
   templates co-located with codes, one doc-table ↔ code-table consistency
   test.
2. 5xxx (functions) — smallest surface, exercises per-argument spans.
3. 1xxx (schema references) — retires the largest old-validator block.
4. 2xxx (types) — retires assignability/condition/binary validators.
5. 3xxx (graph) — retires graph validators; `semantic.rs` reaches zero
   validators here.
6. 4xxx + the 🔨 items (transaction state, unreachable).
7. 6xxx constraint channel + `ParamInference` retirement.
8. 7xxx lints, behind config (lints default-on but individually
   disableable).

Old-engine note: after step 5, `semantic.rs`, `expression.rs` (node half),
and `select_ir.rs` have no callers and are deleted; `tree-sitter` leaves
the workspace crate's dependencies. That is the doneness check inherited
from the AST migration.
