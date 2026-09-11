# Changelog

## Unreleased

Two large bodies of work met here: an August series on the SurrealQL grammar,
LIVE SELECT and the Svelte/embedded-query surfaces, and a correctness and
performance sweep over the engine, the language server and the test suite.
Every rule below that concerns SurrealDB's own behaviour was verified against a
live 3.2.3 server rather than inferred.

### Fixed — the language server no longer grows without bound

Startup and every save copied every document's text once per document and held
all the copies alive at once. On a 509-file workspace that was 887 MB of
resident memory that never came back; it is now 77 MB and flat across saves.
Document text is shared rather than copied, diagnostics publish per document,
and the workspace scan skips build directories and honours the globs in
`surrealguard.toml`.

Two smaller leaks went with it: the semantic-tokens refresh was re-requested on
every publish with no capability gate and no coalescing, so a client that never
answered accumulated pending requests forever; and the server ignored the `exit`
notification, ending only on end-of-input, so an editor that restarted the
server could leave the old process running.

### Fixed — editor latency on large files

Converting a byte offset to an editor position rescanned the document from the
first byte, once per span. It is now a cached per-document line index. On a
3,200-line file, diagnostic conversion went from 475 ms to 2.2 ms and whole-file
inlay hints from 253 ms to 15.6 ms, and the cost per line is flat across file
sizes instead of growing.

### Added — diagnostics

Four new contracts, each engine-verified: a constant written to a field that
violates that field's own `ASSERT` (2038); an aggregate over a column with no
`GROUP` clause, which the engine rejects (4028); a projection under `GROUP BY`
that is neither a group key nor an aggregate, which the engine silently
accumulates into an array (4029); and a parameter read by a `DEFINE FUNCTION`
body that nothing binds (6008), which evaluates to `NONE` and quietly computes
the wrong answer.

Version compatibility (8001, 8002, 8003) now emits, keyed on an optional
`[analysis] surrealdb_version`. Unset means the latest release and nothing
fires. The registry of when each function and syntax feature appeared or was
removed is sourced from SurrealDB's own tags and documentation.

Completeness for contracts that previously covered only part of their surface:
duplicate and missing-target reporting for `INDEX`, `EVENT`, `FUNCTION`,
`PARAM` and `ANALYZER`; `REMOVE` and `ALTER` now update the schema so later
statements see the change; event self-trigger cycles (5010); `OMIT` without a
wildcard projection (4012); and the side-effect-in-computed-field lint widened
beyond `http::`.

Retired, having never had an emission site: 4001, 4002, 4016 and 6006. Retired
numbers are not reused.

### Added — grammar and conformance

SurrealDB's own extracted test queries now parse completely: 320 of 320, with a
further 8 entries that must *not* parse (parser error-handling fragments and a
form the engine reserves) asserted as rejected, so the harness catches
over-acceptance as well as regression. The upstream `surrealql-tree-sitter`
corpus passes 396 of 396.

Newly parsed, each verified against the engine: `%`; prefix `NOT`, `-` and `+`
on any operand; `dec` and `f` numeric suffixes; `s''` strings; `?.`; `WITH
INDEX` on mutations; `INSERT IGNORE`; `KILL $param`; `LIVE SELECT ... FROM
$param`; `array<T, N>`; `COMMENT` clauses; `HNSW`, `DISKANN`, `FULLTEXT` and
`COUNT` index kinds; `RATELIMIT`; `DEFINE SEQUENCE`; `DEFINE ACCESS` and
`DEFINE USER`; the `ACCESS` statement; `EXPLAIN` in prefix position; an
immediately-called closure; and `ORDER BY count`.

Lowering gaps that silently hid bugs are closed: every comparison and
containment operator has its own typed variant and infers `bool`, where the
whole family was previously untyped and unchecked; `=` and `==` are
distinguished; `IF NOT EXISTS` no longer reads as a duplicate definition;
`INSERT ... ON DUPLICATE KEY UPDATE` keeps its row payload; `LIVE SELECT`
lowers its `WHERE` and `FETCH`; and `r'table:id'` is a record id rather than a
regex.

### Changed — internals

One field-projection policy replaces five implementations that disagreed on
`option`, unions and record links; joins route through the lattice; one AST
visitor replaces three hand-rolled walkers; the builtin catalogue is one
structured table that both dispatches calls and drives completion; and host-file
span remapping is one method shared by the CLI, the language server and the
WASM host.

### Added — tests

Whole-pipeline tests moved out of `analysis.rs`, which fell from 6,568 lines to
882. The corpus gained 426 engine-verified statements over previously untested
surface. The negative corpus went from 4 distinct codes at 0.5.3 to 81. Every
deny-level code has a fire test and a near-miss guard. The parser gained
property-based testing over corrupted input, which found a missing identifier
being treated as a real empty name across sixteen lowering sites. Generated
TypeScript is compared against a committed golden and typechecked.

### Known

A full edit-to-diagnostics cycle is superlinear in document size: roughly 63 µs
per line at 200 lines and 752 µs at 3,200. This is the analysis pipeline, not
span conversion — a document with no findings costs the same. Under
investigation.

## 0.4.0

The headline is **type-aware autocomplete**. Alongside it, a large correctness pass:
every rule below that concerns SurrealDB's own behaviour was verified against a live
SurrealDB 3.0.5 server rather than inferred, and several turned out to contradict what
the analyzer previously assumed.

### Breaking — generated types change shape

If you consume generated TypeScript, expect these keys/types to differ. Each is a fix
toward what the engine actually returns.

- **Unaliased call projections are keyed by the bare function name.** `SELECT fn::abc(age)`
  is `{ "fn::abc": … }`, not `{ "fn::abc(age)": … }`; likewise `string::len(name)` →
  `string::len`, `time::now()` → `time::now`. An idiom ending in a method drops the method
  (`name.len()` → `name`). Everything else keeps its source text (`age + 1`,
  `math::abs(age) + 1`). Previously the raw source text was used as the key, so consumers
  indexed a key the runtime never returns and read `undefined`.
- **Rows carry the implicit `id`** (and `in`/`out` on RELATION tables) in wildcard
  projections and every mutation return. Schemas rarely declare `id` explicitly, so most
  row types previously omitted their primary key.
- **`type::field` / `type::fields` expand** into the fields their path arguments name,
  matching the engine.
- **A `.{ … }` destructure over a wrapped link hoists the wrapper to the object**:
  `members.{name}` over `array<record<user>>` is `array<{ name: string }>`, not
  `{ name: array<string> }`.

### Added

- **Type-aware completion** (`completion_provider`). Fields, tables, `$params`, `fn::` and
  builtin paths, `.` members across record links, `.{ }` destructure members, `CONTENT`
  keys, and method sugar. Ranking combines fuzzy matching with **type compatibility as a
  primary signal**, so a param whose kind fits the position outranks a closer name match.
  Served entirely from the analysis cache — no re-analysis or re-parse per keystroke.
  - Graph slots offer only what the receiver can actually traverse, direction-aware and
    multi-hop; `GROUP BY` offers only what can label a group; a graph step's filter
    completes the step's own fields; `WITH INDEX` offers that table's indexes, and
    full-text functions are withheld where the schema proves no such index exists.
- **New diagnostics**: `4025` (a wildcard under `GROUP` — the engine rejects the query),
  plus `4013`, `6002` and `7001` (opt-in), which were catalogued but never emitted.
- `check` now scans host files (`.ts`/`.tsx`/`.svelte`/…), so embedded queries are covered
  by CI. Previously a workspace whose `generate` failed could pass `check` with exit 0.

### Fixed

- **`??` strips `NONE` from an option left operand.** `(optional ?? 'default') + '!'` raised
  a false `E2004` whose help told you to coalesce — which you had.
- **Method dispatch** resolves what the engine accepts (`to_string`, `type_of`, `diff`,
  `patch`, `repeat`, the whole `is_*` family). An unresolved method is reported as `E5001`,
  so these were error-severity false positives that aborted `generate` for a whole project.
- **`GROUP BY` / `ORDER BY` over a projection alias** no longer reports a false `E1002`.
- **Aggregate columns** promote for any argument expression and at any depth in a
  computation, instead of only a bare `math::x(field)`.
- **An opaque `CONTENT $payload`** no longer claims every required field is missing.
- **Descendant field definitions refine the parent's kind rather than replacing it**, so
  `array<object>` + `[*].price` and `option<object>` + subfields keep their declared shape
  (and their `DEFAULT`). Paths into a refined parent still resolve.
- **Wrapped record links traverse**: `option<record<T>>`, `array<record<T>>` and `set<…>`
  resolve through the link and keep their wrapper, instead of degrading to `unknown`.
- **Array-form `INSERT` payloads are checked.** `INSERT INTO t [{ … }]` was discarded by the
  lowerer, so required fields, unknown keys and value kinds went entirely unvalidated.
- **A wildcard under `GROUP` no longer fabricates** the non-grouped fields, which promised
  fields that do not exist at runtime.
- `COMPUTED`/`VALUE` fields reading a sibling `$this.<field>` no longer degrade to `any`.
- `FOR` over a union-wrapped collection types its loop binding; index/method operations
  distribute over a union; an empty-array `RETURN` no longer widens a block's return type.
- Writing a wrong literal to a literal-union field errors again, naming the value.
- `check --json` keeps warnings on a clean run (the summary counted them; the array was empty).

### Performance

- **LSP incremental analysis.** A query-file edit re-analyzes only that file against a
  cached schema (~96× on the measured corpus). A schema edit uses symbol-level
  invalidation — a function-body edit re-analyzes the edited file plus only what
  references a changed symbol, rather than the whole workspace (1.70s → 256ms on a
  149-file corpus; ~30ms per edit in practice).
