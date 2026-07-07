# Diagnostic catalog: categories, codes, and the parameter-constraint channel

Status: catalog v1 complete (four iteration rounds, 2026-07-07); awaiting
severity stamps. This is the canonical registry the diagnostics phase
implements against; codes are assigned here and only here, append-only.

## Principles (settled in prior rulings)

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
| compat | 8xxx | SurrealDB version compatibility |

Severity defaults: **error** = provably fails or misbehaves at runtime;
**warning** = provably suspicious but executable; **info** = analyzer
limitation or style note.

Severity is the finding's *intrinsic class*; it is data, not policy.
Consumers apply policy at their edge through `PolicyConfig`
(`warnings_as_errors` promotion, per-code allow/warn/deny for lints,
suppression) — the CLI for exit codes, the LSP for editor severities, host
adapters for build failures. Errors are never demotable; lints are fully
configurable. (Ruled 2026-07-07; already implemented at the CLI and LSP
edges.)

Detection status legend: ✅ inference already computes everything needed
(emission is additive); 🔶 partial (needs modest new analysis at the site);
🔨 needs new analysis machinery (named).

---

## 1xxx — Schema references

| Code | Finding | Example | Sev | Status |
|---|---|---|---|---|
| 1001 | unknown table in FROM/target | `SELECT * FROM ghost` | E | ✅ |
| 1002 | unknown field in projection | `SELECT nickname FROM person` | E | ✅ |
| 1003 | unknown field in WHERE/expression | `... WHERE nickname = 'x'` | E | ✅ |
| 1004 | unknown field as SET/UNSET target | `UPDATE person SET aeg = 1` | E | ✅ |
| 1005 | unknown field as CONTENT/MERGE key | `CONTENT { aeg: 1 }` | E | ✅ |
| 1006 | unknown field in RETURN projection | `RETURN aeg` | E | ✅ |
| 1007 | unknown field in OMIT | `OMIT aeg` | E | ✅ |
| 1008 | unknown field in FETCH | `FETCH aeg` | E | ✅ |
| 1009 | unknown field in SPLIT | `SPLIT aeg` | E | ✅ |
| 1010 | unknown field in GROUP/ORDER BY | `ORDER BY aeg` | E | ✅ |
| 1011 | unknown INSERT tuple column | `INSERT INTO person (aeg) ...` | E | ✅ |
| 1012 | unknown index target | `REBUILD INDEX ghost ON person` | E | 🔶 index registry exists in extraction |
| 1013 | unknown event target | `REMOVE EVENT ghost ON person` | E | 🔶 |
| 1014 | unknown analyzer reference | index `SEARCH ANALYZER ghost` | E | 🔶 |
| 1015 | unknown user function | `RETURN fn::ghost()` | E | ✅ |
| 1016 | `record<t>` names unknown table | `DEFINE FIELD f ... TYPE record<ghost>` | E | ✅ |
| 1017 | relation endpoint names unknown table | `DEFINE TABLE e TYPE RELATION IN ghost OUT post` | E | ✅ |
| 1018 | DEFINE FIELD/INDEX/EVENT on unknown table | `DEFINE FIELD x ON ghost` | E | ✅ |
| 1019 | index field not on table | `DEFINE INDEX i ON person FIELDS aeg` | E | ✅ (exists today) |
| 1020 | event WHEN/THEN references unknown field | | E | 🔶 |
| 1021 | REMOVE of object that doesn't exist | `REMOVE TABLE ghost` | W | ✅ (exists today) |
| 1022 | duplicate definition without OVERWRITE | two `DEFINE TABLE person` | W | 🔶 extraction sees both |
| 1023 | FETCH of a non-record field | `FETCH age` (int) | E | ✅ kinds known |
| 1024 | SPLIT of a non-collection field | `SPLIT age` | E | ✅ |
| 1025 | subfield declared under a non-object field | `FIELD a TYPE int` then `FIELD a.b` | E | ✅ extraction sees both |
| 1026 | use of a table/field after its REMOVE earlier in the script | `REMOVE TABLE t; SELECT * FROM t` | E | ✅ source-order effects exist |
| 1027 | full-text operator/function without a search index | `WHERE text @@ 'x'`, `search::score(1)` with no `SEARCH ANALYZER` index on the field | E | 🔶 index registry lookup |
| 1028 | KNN operator without a vector index | `<\|4\|>` with no MTREE/HNSW index | E | 🔶 same |
| 1029 | duplicate index over identical fields | two indexes on `(email)` | W | 🔶 |
| 1030 | WITH index hint names unknown index | `WITH INDEX ghost` | E | 🔶 lower `WithClause` |
| 1031 | PATCH path names unknown field | `PATCH [{op:'replace', path:'/aeg', ..}]` | E | 🔶 walk const patch objects |
| 1032 | unknown tokenizer/filter/snowball language in DEFINE ANALYZER | `TOKENIZERS blanc`, `FILTERS snowball(klingon)` | E | 🔶 lower analyzer definitions |

## 2xxx — Types and nullability

| Code | Finding | Example | Sev | Status |
|---|---|---|---|---|
| 2001 | SET value not assignable to field | `UPDATE person SET age = 'string'` | E | ✅ |
| 2002 | CONTENT/MERGE value not assignable per key | `CONTENT { age: 'x' }` | E | ✅ |
| 2003 | INSERT tuple value not assignable to column | `(age) VALUES ('x')` | E | ✅ |
| 2004 | operand kinds incompatible for operator | `name > 18`, `1 + 'a'` | E | ✅ (`binary_operands_compatible`) |
| 2005 | IF condition not boolean | `IF 'yes' { }` | W | ✅ |
| 2006 | WHERE clause not boolean-ish | `WHERE age` (truthiness works; style) | I | ✅ |
| 2007 | cast to unknown type | `<ghost> x` | E | ✅ |
| 2008 | cast that cannot succeed | `<duration> true` | W | 🔶 castability table |
| 2009 | DEFAULT not assignable to declared type | `TYPE int DEFAULT 'x'` | E | 🔶 lower DefaultClause |
| 2010 | VALUE clause not assignable to declared type | | E | 🔶 lower ValueClause |
| 2011 | ASSERT not boolean | `ASSERT $value + 1` | E | 🔶 lower AssertClause |
| 2012 | fn:: body return doesn't match declared `->` type | `-> string { RETURN 1 }` | E | 🔨 body analysis vs declaration (body already lowers) |
| 2013 | closure body vs declared return mismatch | `\|$x\| -> string { RETURN 1 }` | E | ✅ closure inference exists |
| 2014 | negation of non-numeric | `-name` (string field) | E | ✅ |
| 2015 | possibly-NONE value where value required | `option<int>` field in `x + 1` | W | 🔶 Either[None, T] flows exist; needs the operand rule |
| 2016 | assigning NONE to non-optional field | `SET age = NONE` | E | ✅ |
| 2017 | ORDER BY on non-comparable kind | `ORDER BY tags` (array) | W | ✅ |
| 2018 | LIMIT/START not an integer | `LIMIT 'a'` | E | ✅ |
| 2019 | TIMEOUT not a duration | `TIMEOUT 5` | E | ✅ |
| 2020 | KILL argument not a uuid | `KILL 42` | E | ✅ |
| 2021 | SHOW SINCE not versionstamp/datetime | | E | ✅ |
| 2022 | FOR over a non-iterable | `FOR $x IN 42 { }` | E | ✅ iterable kind known |
| 2023 | const conversion provably fails | `<int> 'abc'`, `type::int('x')` | E | ✅ const tracking evaluates it |
| 2024 | negative const LIMIT/START | `LIMIT -1` | E | ✅ const tracking |
| 2025 | assignment to a READONLY field | `UPDATE t SET created = ...` | E | 🔶 lower `ReadonlyClause` |
| 2026 | assignment to a computed (VALUE-clause) field | value will be overwritten | W | 🔶 lower `ValueClause` |
| 2027 | record link targets the wrong table | `SET friend = post:1` when `friend: record<user>` | E | 🔶 record-target assignability rule |
| 2028 | VERSION clause not a datetime | `VERSION 5` | E | 🔶 lower `VersionClause` |
| 2029 | compound assignment incompatible with field kind | `SET age += 'x'`; `tags -= 42` on `array<string>` | E | 🔶 operator-aware assignability (`+=` is push on arrays, add on numbers, concat on strings) |
| 2030 | index/filter/splat on a non-collection | `age[0]`, `name[WHERE ..]`, `age.*` | E | ✅ idiom stepping knows the kind |
| 2031 | invalid const regex pattern | `name ~ 'unclosed('` | E | ✅ const tracking + regex compile |
| 2032 | invalid literal content | `d'2024-13-45'`, `u'not-a-uuid'`, duration `5x` | E | 🔶 validate the slice at the literal site |
| 2033 | invalid PATCH operation | `{op: 'remvoe', ...}`, path without `/` | E | 🔶 const patch objects |
| 2034 | missing required field on CREATE/CONTENT/INSERT | `CREATE person;` when `name: string` has no DEFAULT and isn't optional | E | 🔶 required-field metadata in extraction |
| 2035 | invalid analyzer filter arguments | `edgengram(5, 2)` (min > max) | E | 🔶 same lowering as 1032 |
| 2036 | invalid GeoJSON literal shape | `{type: 'Pointt', ...}`, Point with 3-ring coordinates | E | ✅ const objects carry the shape |

## 3xxx — Graph and relations

| Code | Finding | Example | Sev | Status |
|---|---|---|---|---|
| 3001 | traversal edge is not a relation table | `->person->` | E | ✅ |
| 3002 | relation does not connect these tables in this direction | `user->writes->user` when `writes` relates user→post | E | ✅ (`relation_step_target` already resolves) |
| 3003 | traversal target unreachable from edge | `->writes->comment` when OUT is post | E | ✅ |
| 3004 | dangling edge hop (edge without target where one is required) | `SELECT ->writes FROM user` is valid; `FROM user->writes` is not | E | ✅ odd-chain detection exists |
| 3005 | multi-target step cannot resolve | `->(a, b)->x` | W | ✅ |
| 3006 | RELATE endpoints violate the relation's IN/OUT | `RELATE post:1->writes->user:1` | E | ✅ endpoints + relation known |
| 3007 | RELATE edge is not a relation table | `RELATE a->person->b` | E | ✅ |
| 3008 | RELATE endpoint table unknown | `RELATE ghost:1->writes->post:1` | E | ✅ |
| 3009 | graph step off a non-record position | `age->writes->` | E | ✅ |
| 3010 | FETCH alias that is not a record target | alias of a computed value | W | ✅ |
| 3011 | unbounded graph recursion | `@{..}` / recurse range with no upper bound | W | 🔶 `Recurse` nodes lower as partials today |

## 4xxx — Statement and clause misuse

| Code | Finding | Example | Sev | Status |
|---|---|---|---|---|
| 4001 | clause not valid on this statement | `SELECT ... RETURN NONE`, `CREATE ... WHERE` | E | ✅ lowering already isolates them |
| 4002 | SELECT VALUE with multiple projections | `SELECT VALUE a, b FROM t` | E | ✅ |
| 4003 | ONLY on a multi-row target | `CREATE ONLY person` (whole table) | W | ✅ |
| 4004 | INSERT tuple column/value count mismatch | `(a, b) VALUES (1)` | E | ✅ lowering counts |
| 4005 | BREAK/CONTINUE outside a loop | top-level `BREAK` | E | 🔶 loop-depth flag on ctx |
| 4006 | unreachable statements after RETURN/BREAK/THROW | `RETURN 1; SELECT ...` in a block | W | 🔶 block walk already sequential |
| 4007 | COMMIT/CANCEL without BEGIN | | E | 🔨 transaction state in script walk |
| 4008 | nested BEGIN | | E | 🔨 same |
| 4009 | LIVE SELECT with unsupported clause | `LIVE SELECT ... GROUP BY` | E | 🔶 LiveSelect lowering keeps clauses |
| 4010 | duplicate SET target in one statement | `SET age = 1, age = 2` | W | ✅ assignments are structured |
| 4011 | duplicate projection key/alias | `SELECT age, age FROM t`, two `AS x` | W | ✅ keys computed |
| 4012 | OMIT without a wildcard projection | `SELECT age OMIT age` | W | ✅ |
| 4013 | GROUP BY field not in projections | SurrealDB aggregate rules | W | 🔶 verify exact semantics first |
| 4014 | RETURN outside a function/block context where invalid | | W | 🔶 |
| 4015 | BEGIN never closed | `BEGIN;` with no COMMIT/CANCEL by script end | E | 🔨 transaction state (same walk as 4007/4008) |
| 4016 | empty block | `IF x { }` | I | ✅ |
| 4017 | block ends with LET — its value is NONE | `{ LET $x = f(); }` consumed as a value | W | ✅ block value known |
| 4018 | side-effecting subquery in read position | `SELECT (CREATE log) FROM t` | W | ✅ statement kinds known |
| 4019 | CREATE/INSERT on a relation table without `in`/`out` | `CREATE likes SET strength = 1` | W | ✅ relation-ness known |
| 4020 | RETURN mode meaningless for the statement | `CREATE ... RETURN BEFORE` (always NONE) | W | ✅ (verify DELETE/AFTER semantics first) |
| 4021 | SHOW CHANGES on a table without CHANGEFEED | | E | 🔶 changefeed flag in extraction |
| 4022 | SELECT from a DROP table | rows are never retained | W | 🔶 drop flag in extraction |

## 5xxx — Functions and closures

| Code | Finding | Example | Sev | Status |
|---|---|---|---|---|
| 5001 | unknown function | `RETURN string::lenght('x')` | E | ✅ dispatch fallthrough |
| 5002 | wrong argument count | `string::len()` | E | ✅ Signature.min/max |
| 5003 | argument kind mismatch (per-argument span) | `string::len(42)` | E | ✅ Signature.arg_kinds + call spans |
| 5004 | closure parameter count wrong for consumer | `array::map(a, \|$x, $y, $z, $w\| ...)` | W | ✅ invoke arity known per consumer |
| 5005 | const value-dependent violation | `type::field('aeg')` — no such field | E | ✅ const tracking resolves it |
| 5006 | fn:: argument count/kind vs its DEFINE | `fn::greet()` when it takes `$name: string` | E | ✅ params extracted with kinds |
| 5007 | non-string idiom in value-dependent position | `type::field(42)` | E | ✅ |
| 5008 | method not available on receiver kind | `age.uppercase()` | E | ✅ method dispatch knows |
| 5009 | fn:: recursion cycle (direct or mutual) | `fn::f` calls `fn::f` | W | 🔨 call graph over lowered bodies (bodies already lower) |
| 5010 | event trigger cycle | event on `person` THEN mutates `person` (or A→B→A) | W | 🔨 event-effect graph |
| 5011 | const table argument names unknown table | `type::thing('ghost', $id)` | E | ✅ const tracking + schema |
| 5012 | const argument outside the function's valid range | `math::fixed(x, -1)`, `string::slice(s, 0, -5)` | W | 🔶 per-function range metadata, grown lazily |

## 6xxx — Parameters

| Code | Finding | Example | Sev | Status |
|---|---|---|---|---|
| 6001 | conflicting constraints on one param | `WHERE $x > 3 AND $x = 'abc'` | E | 🔶 constraint unification |
| 6002 | param shadows a DEFINE PARAM with a different kind | `LET $min_age = 'x'` vs defined int | W | 🔶 |
| 6003 | unresolvable dynamic construct (analyzer limitation) | current `dynamic(6001)` class | I | ✅ |
| 6004 | param used before its LET in source order | `RETURN $x; LET $x = 1;` | W | ✅ env is source-ordered |
| 6005 | context param used outside its context | `$before` outside an event, `$parent` outside a subquery | E | 🔶 context-param model below |
| 6006 | host-declared type contradicts query constraint | host binds `$age: string`, query needs int | E (host-static) | 🔨 adapter layer; registered here so it is never dropped |
| 6007 | assignment to a protected parameter | `LET $auth = {...}` | E | ✅ protected-name list ($auth, $session, $token, $this, ...) |

## 7xxx — Lints

| Code | Finding | Example | Sev | Status |
|---|---|---|---|---|
| 7001 | unused LET binding | `LET $x = 1;` never read | W | 🔶 use-tracking exists in env |
| 7002 | LET shadowing | inner `LET $x` over outer | I | ✅ scopes exist |
| 7003 | mixed-kind array literal | `[1, 'a']` | I | ✅ (today's partial fact) |
| 7004 | condition is constant | `IF true`, `WHERE 1 = 1` | W | ✅ const tracking |
| 7005 | comparison always false by kind | `age = 'x'` (int vs string, `=` runs but never matches) | W | ✅ |
| 7006 | empty IN/CONTAINS list | `WHERE x IN []` | W | ✅ const arrays |
| 7007 | SELECT * with explicit fields | `SELECT *, age FROM t` | I | ✅ |
| 7008 | schemaless table in a typed workspace | queries against fieldless tables | I | ✅ |
| 7009 | whole-table UPDATE/DELETE without WHERE | `DELETE person;` | W | ✅ (deliberate ones silence per-code) |
| 7010 | FOR over an empty const collection | `FOR $x IN [] { ... }` | W | ✅ const tracking |
| 7011 | assignment to `id` in SET | `SET id = ...` | W | ✅ |
| 7012 | blocking or side-effecting call in a computed context | `http::get(...)` / `sleep()` in a field `VALUE` or event body | W | ✅ call paths known |

## 8xxx — Version compatibility

The workspace config gains `surrealdb_version`; checks gate on it. Needs a
small version registry (function → introduced/removed/renamed-in), built
from the surrealdb source the same way the signature table was.

| Code | Finding | Example | Sev | Status |
|---|---|---|---|---|
| 8001 | function not available in the configured version | `array::fold` on a 1.x target | E | 🔨 version registry |
| 8002 | function renamed in the configured version | `string::endsWith` → `string::ends_with` (suggests the rename) | E | 🔨 same |
| 8003 | syntax requires a newer version | closures / `??` on old targets | E | 🔨 same |

**Count: 135 codes across 8 families**, of which **87 are ✅ ready** (the
detection already exists inside inference or lowering; emission is
additive), 38 🔶 need modest site work, 10 🔨 need a named new capability
(transaction state, call/event graphs, fn:: body-vs-declaration,
castability table, constraint unification, index-dependency registry,
version registry, the adapter layer). The catalog is append-only within
each family.

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

```rust
pub struct ParamConstraint {
    pub name: String,
    pub kind: Kind,                       // unified across uses
    pub domain: Option<ValueDomain>,      // beyond the kind, when known
    pub origins: Vec<SourceSpan>,         // every contributing use
}

pub enum ValueDomain {
    /// Enumerable values (field paths for `type::field`, table names).
    OneOf(Vec<Value>),
    /// Numeric range (`LIMIT $n` → int, `0..`).
    Range { min: Option<Value>, max: Option<Value> },
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
- This subsumes today's `ParamInference` (kind-only, old engine); the
  constraint collector is its AST-native replacement and retires it.

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
