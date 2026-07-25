# Changelog

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
