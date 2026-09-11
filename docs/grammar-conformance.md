# Grammar conformance report (2026-09-11)

**Both corpora are at 100%.** There is no expected-failure baseline any more;
`crates/syntax/tests/conformance_expected_failures.txt` is gone.

| corpus | entries | result |
| --- | --- | --- |
| valid set (`crates/syntax/examples/conformance_corpus.json`) | 320 | 320 parse, 0 fail |
| rejected set (`crates/syntax/examples/conformance_rejected.json`) | 8 | 8 refused, 0 wrongly accepted |
| upstream `surrealql-tree-sitter` `test/corpus/*.txt` (head `22feaab`) | 396 | 396 parse |

Both sets are extracted from SurrealDB's own test suites, except for six
entries (five valid, one rejected) added here to pin the `ORDER BY count`
work below. Those six were each run against a live 3.2.3 first; an entry
that is not from SurrealDB's suites earns its place by engine evidence, not
by assertion.

Runner: `cargo run -p surrealql-analyzer-syntax --example conformance`
(add a path to check one file as a valid set only).
Gate: `cargo test -p surrealql-analyzer-syntax --test conformance`.

## The valid/rejected split

The corpus used to be one list of "known-valid SurrealQL" held against a
baseline of entries that still failed. That conflated two very different
things, and the baseline hid both: a real grammar gap and a string that was
never SurrealQL sat on the same line, and the only thing the gate could say
was "still failing".

There are now two committed corpora, and the gate fails in **both**
directions:

- **The valid set** — `conformance_corpus.json`, a JSON array of query
  strings. Every entry must parse cleanly. A parse error is fatal to the
  whole source (the analyzer sees a `Partial` statement and says nothing),
  so an entry the grammar rejects is a place the analyzer is silently wrong
  on valid input. **Fix the grammar, never the corpus.**
- **The rejected set** — `conformance_rejected.json`, a JSON array of
  `[query, reason]` pairs. Text that came out of the same test suites but is
  not SurrealQL. Every entry must fail to parse. An entry that starts parsing
  is **over-acceptance**: the grammar grew looser than the language, and text
  SurrealDB itself refuses would reach the analyzer as if it were a real
  query. That check did not exist before.

Every rejected entry carries a reason, a test asserts none is empty, and a
test asserts the two sets are disjoint. There is no `UPDATE_SNAPSHOTS` path:
at 100% in both directions the invariant is absolute, so there is nothing to
re-record. Moving an entry between the sets is a deliberate edit with a
reason, not a regeneration.

## What the 20 former failures actually were

The 2026-09-08 report listed 21 failures (one, `#85`, had since been banked)
and called them all "junk extractions". They were three different things.

**Thirteen were valid SurrealQL the extractor mangled** by keeping a Rust
source line-continuation — a trailing `\` (or, for two entries, a literal
two-character `\n`). Each was cross-checked against SurrealDB's own sources
at rev `c7eac9022` and repaired to what SurrealDB actually tests:

| # | source | repair |
| --- | --- | --- |
| 169 | `core/tests/define.rs:553` | strip trailing `\` |
| 177 | `core/tests/define.rs:724` | strip trailing `\` |
| 180 | `core/tests/define.rs:774` | strip trailing `\` |
| 186 | `core/tests/remove.rs:994` | strip trailing `\` |
| 189 | `core/tests/remove.rs:1079` | strip trailing `\` |
| 192 | `core/tests/define.rs:1086` | strip trailing `\` |
| 198 | `core/tests/define.rs:1165` | strip trailing `\` |
| 203 | `core/tests/index.rs:237-239` | collapse the continuations (see below) |
| 242 | `core/tests/complex.rs:134` | strip literal `\n` |
| 243 | `core/tests/complex.rs:135` | strip literal `\n` |
| 291 | `core/tests/define.rs:811` | strip trailing `\` |
| 292 | `core/tests/define.rs:905` | strip trailing `\` |
| 293 | `core/tests/define.rs:994` | strip trailing `\` |

Entry 203 is the interesting one: a `\` before a newline in a Rust string
literal eats the newline *and the next line's leading whitespace*, so the
string SurrealDB parses is the three statements run together with no
separator at all —
`INFO FOR INDEX field1 ON aaa;count(…);SELECT VALUE [field1, count] FROM (…)`.
The corpus now holds that.

Twelve of the thirteen parsed as soon as the continuation was stripped: the
gaps the old report implied (`DURATION FOR TOKEN …, FOR SESSION …`,
`PERMISSIONS FULL` on `DEFINE PARAM`/`FUNCTION`, `DEFINE EVENT … THEN
RETURN`, `INFO FOR INDEX … ON`) had all been closed already, and only the
backslash was keeping them red. Entry 203 exposed **one real grammar gap**:
`count` was a function-name token everywhere, so it could not be a field
name. `SELECT field1, count() FROM t GROUP field1` names its aggregate
column `count`, and `SELECT VALUE [field1, count] FROM (…)` reads it back —
verified on 3.2.3. `count` joined `_nonReservedIdent` beside
`order`/`start`/`limit`/`group`/`key`.

**Three were regex assertions, not queries** (#283, #285, #287):
`DEFINE USER user ON <level> PASSHASH .* ROLES VIEWER`, where `.*` matches
the password hash in `INFO` output (`core/tests/info.rs:104/119/134`). The
real *input* queries behind them live a few lines above
(`core/tests/info.rs:89-91`) and were restored to the valid set:
`DEFINE USER user ON ROOT|NS|DB PASSWORD 'pass';`. The `.*` strings moved to
the rejected set.

**Four were deliberate fragments** from SurrealDB's parser *error-handling*
tests — `}`, `a:[`, `]`, `SELECT * FROM` (#130, #131, #133, #244). They are
meant not to parse, so they moved to the rejected set. Making the grammar
accept them would have made us wrong.

Net for this step: 319 entries → 315 valid (4 fragments out) + 7 rejected
(4 fragments + 3 regex assertions), with 3 restored queries taking the place
of the regex assertions. The six `ORDER BY count` entries added afterwards
bring the committed sets to their current 320 and 8.

## Where the grammar is deliberately looser than the engine

Three upstream corpus cases are forms SurrealDB 3.2.3 refuses outright:

| form | engine error |
| --- | --- |
| `KILL "plain-string"` | ``Unexpected token `a strand`, expected a UUID or a parameter`` |
| `SHOW CHANGES FOR TABLE person` | ``Unexpected token `;`, expected SINCE`` |
| `SHOW CHANGES FOR TABLE person LIMIT 10` | ``Unexpected token `LIMIT`, expected SINCE`` |

We used to refuse them too. For a language server that is the wrong trade: a
parse error is fatal to the whole source, so refusing one statement silences
the analyzer on the entire file, and all the user gets is a token-level
syntax error. The grammar now accepts all three and the analyzer names the
contract at the span that is actually wrong — **E2020** ("KILL takes a
live-query uuid") and **E2021** ("SHOW SINCE takes a versionstamp or
datetime"), each quoting the engine's own error text in its help so the user
sees what SurrealDB will say. `crates/workspace/tests/engine_refused_syntax.rs`
pins that every one of them fires, and that the correctly-spelled neighbour
(`KILL u'…'`, `KILL $id`, `SHOW … SINCE …`) stays silent.

Uuid *shape* makes no difference: 3.2.3 rejects
`KILL "018e0f3a-1234-7abc-8def-0123456789ab"` exactly as it rejects
`KILL "not-a-uuid"`. Only a `u'…'` literal or a parameter is a live-query id.

This also makes our grammar a strict superset of upstream
`surrealql-tree-sitter`, which matters because upstream's consumers are
meant to be able to adopt this grammar without their corpus tests
regressing. Node names for the shared index-kind clauses (`CountClause`,
`FullTextClause`, `DiskAnnClause`, `DiskAnnDistClause`) match upstream's for
the same reason.

## `ORDER BY count`

Making `count` a legal field name in value position left an inconsistency:
the grammar parsed the field but not an `ORDER BY` over it, which is a false
error on ordinary SurrealQL — a table with a `count` column is nothing
unusual, and `SELECT field1, count() FROM t GROUP field1 ORDER BY count
DESC` is the natural way to read the aggregate back.

Every member of `_nonReservedIdent` was checked in order position against a
live 3.2.3 before anything was admitted, plus `rand`:

| spelling | engine | grammar before | now |
| --- | --- | --- | --- |
| `ORDER BY count` | accepted | rejected | **accepted** |
| `ORDER BY count.total` | accepted | rejected | **accepted** |
| `ORDER BY order` / `key` / `start` / `limit` / `group` | accepted | accepted | accepted |
| `ORDER BY rand` | **rejected** — ``Unexpected token `;`, expected (`` | rejected | rejected |

`rand` is genuinely reserved there: the engine takes only the `ORDER BY
RAND()` call form, so refusing a bare `rand` is correct strictness, and it
is pinned in the rejected corpus so a future loosening cannot swallow it by
accident. `order`/`key`/`start`/`limit`/`group` already worked — the
function-name keywords are the only ones `Idiom` cannot reach in this state,
because the clause's own `ORDER BY RAND()` alternative keeps them live as
tokens.

The fix is a hidden `_countIdiom` — an idiom rooted at `count`, aliased to
`Idiom` so the CST shape and every consumer are unchanged — offered beside
`$.Idiom` in `Order`. The idiom tail is factored into a hidden `_idiomTail`
shared by both, so the two cannot drift. `tree-sitter generate` reports no
conflict; no precedence annotation and no `conflicts` entry were needed. The
lowering test `lowers_order_by_count_as_a_field_not_a_call` pins that the
order key is the field path, not a call, and
`order_by_bare_rand_stays_a_parse_error` pins the other half.
