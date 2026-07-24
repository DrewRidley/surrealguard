# SurrealGuard pre-publish findings & fix plan (2026-07-24)

Consolidates: the 4-surface e2e usability swarm (Next.js, SvelteKit, generic
JS/TS, Rust proc-macro), the live `$auth`/E6001 analyzer bug, the new
"disallow `any`" opt-in lint request, and the LSP hover polish
(`docs/plans/2026-07-24-lsp-hover-polish.md`). Goal: a publish-ready release.

**Hard constraint:** do NOT edit `crates/workspace/src/query.rs` or
`crates/lsp/src/backend.rs` — another agent owns the hover reformat there.

**Status of the folded sources:** all four e2e reports are in (Next.js,
SvelteKit, generic-JS, Rust). SvelteKit confirmed the same `db.query` on-ramp
defect and added Svelte-specific findings (folded below).

---

## Workstream A — TypeScript on-ramp + CLI (the biggest gap)

Verdict from Next.js + generic-JS agents: the type contract is excellent and
sound (all negative cases rejected under real `tsc`, no permissive string
overload), but the **documented on-ramp does not work** and the CLI's CI story
is unsafe. Three converging defects, all in `crates/embed/src/typescript.rs`
and `crates/cli/src/main.rs` (NOT the forbidden files):

1. **`db.query("...")` is not a recognized codegen/LSP sink (critical).**
   `is_surql_tag` (`crates/embed/src/typescript.rs:~53`) only matches a call
   or tag literally named `surql`, and `@surrealguard/client` exports no
   `surql` helper at all. So `surrealguard generate` produces an **empty
   registry** and every documented `db.query(...)` result resolves to
   `unknown[]` (TS18046). This is the TS-runtime plan's own unimplemented TODO
   (`docs/plans/2026-07-15-typescript-runtime.md:256-259`) — but the READMEs
   present it as shipping. **Fix:** implement `db.query`/`query`/configurable
   call-name recognition in `is_surql_tag`/`collect`, keyed to the client's
   actual API. (Registry keys match by literal string text, so once a literal
   is recognized the param/result typing already works.)

2. **`surrealguard generate` swallows analyzer diagnostics (high).** A bad
   field in an embedded query is emitted as `field: unknown` with exit 0 — no
   warning. `run_generate` reuses `analyze_workspace` output but never surfaces
   `output.diagnostics`. **Fix:** print diagnostics and exit non-zero on error.

3. **`surrealguard check` never scans embedded TS/JS queries (high).**
   `run_check` (`crates/cli/src/main.rs:~167`) only walks `.surql` sources; the
   host-file scan lives only in `run_generate` (`~291-325`). So a CI gate on
   `check` cannot catch a typo'd inline query — only the LSP does. **Fix:**
   reuse the host-file scan in `run_check`.

Secondary:
4. `examples/surrealguard.toml` uses a stale schema (`[schema].path`,
   `[output]`, …) the CLI silently ignores; real keys are
   `[sources]/[analysis]/[diagnostics]/[lints]` (`EXAMPLE_CONFIG` in
   `crates/cli/src/main.rs`). Align the example file; the existing test only
   checks the inline `EXAMPLE_CONFIG`, so the drift went undetected.
5. Default `generate --out` is the workspace root (`crates/cli/src/main.rs:~344`),
   breaking the READMEs' `import … from "./surrealguard.generated"`; default to
   a `src/`-relative path when `src/` exists, or document the `--out` flag.
6. `.surql` query files under `queries/**` never feed codegen (only host-file
   embeds do). Document, or support them.
7. Missing-required-params surfaces a confusing `TS2345 … BoundQuery<unknown[]>`
   instead of naming the param (`packages/client/src/client.ts` overload set).
8. Package versions (0.2.0) trail npm (0.3.0) — bump before publish.

### SvelteKit-specific (Workstream A additions)
9. **README example fails in Svelte 5 (critical for Svelte users).**
   `packages/svelte/README.md` and `packages/svelte/src/index.ts`'s doc comment
   show `export let data;` — Svelte-4 legacy syntax that fails to compile
   (`Cannot use 'export let' in runes mode`) in any fresh SvelteKit app. Fix to
   `let { data } = $props();`, and fix the `liveQuery(..., { initial: data.users })`
   example, which trips Svelte 5's `state_referenced_locally` warning (captures
   only the initial snapshot — a real reactivity footgun).
10. **`liveQuery` is completely untyped (flagship feature).**
   `@surrealguard/svelte`'s `liveQuery(client, sql: string, options)` takes a
   plain `string` unlinked to `SurqlRegistry`; `Row` defaults to
   `Record<string, unknown>`. So any field access / any params type-check
   silently. Two halves: (a) wire `liveQuery` to consume `SurqlRegistry` like
   `db.query` (`packages/svelte`, Workstream A); (b) LIVE SELECT projection +
   param inference so codegen emits a real result type instead of
   `result: string` — that half is analyzer work (`live_select.rs`), assigned
   to **Workstream C** (see C5). A depends on C for the emitted type.
11. Document `generate --out src/lib/surrealguard.generated.ts` + `$lib` alias
   for SvelteKit (root-relative default forces fragile `../../` imports).

---

## Workstream B — Rust adapter (`crates/rs` + `crates/macros`)

Verdict: works-with-friction. Both directions of the core promise hold — a
correct `query!()` compiles to a typed nameless struct with nested access; a
wrong query fails `cargo build` with a good `[Exxxx]` message (multi-finding
aggregation works). Both crates build/test clean and ARE workspace members.
Gaps:

1. **`RecordLink<T>` is dead code.** `crates/macros/src/generate.rs:57` maps
   `Kind::Record(_) => String`, so record-link fields silently type as
   `String`, contradicting `crates/rs/README.md` and
   `docs/plans/2026-07-15-rust-adapter.md:53`. Wire it to the (already
   scaffolded) `RecordLink<T>` in `crates/rs/src/lib.rs:63-73`, or downgrade
   the docs until implemented.
2. **"Schemaless" mode doesn't degrade as documented.** Any `FROM <table>`
   with no matching `DEFINE TABLE` is `[E1001] unknown table`, so schemaless
   only works for table-free queries. Either suppress `E1001` when the
   workspace has zero schema sources, or fix the docs
   (`crates/rs/README.md:34-36`, `docs/plans/2026-07-15-rust-adapter.md:82`).
3. **`SURREALGUARD_SCHEMA` env switch → stale cached build.** File-content
   edits rebuild correctly (`include_bytes!`), but repointing the env var
   doesn't invalidate Cargo's fingerprint (inherent to proc-macros). Document
   the `cargo clean` caveat near schema resolution.
4. `crates/macros` has no `README.md` but `readme.workspace = true` → will
   break `cargo publish`. Add one (release prep; pre-existing across crates).
5. No `trybuild`/UI compile-fail test guards the macro's `compile_error!`
   path — add one in `crates/macros/tests/`.

---

## Workstream C — Analyzer: `$auth`/E6001, control-flow, disallow-`any`, casting

### C1. `$auth` / context-param false positive (the live bug)
Repro (inside a `DEFINE FUNCTION` body):
```surql
IF $auth = NONE OR $organization = NONE THEN RETURN false END;
...
LET $direct = SELECT role, unit FROM employee_of
    WHERE out = $organization AND in = $auth;
```
fires `[E6001] `$auth` cannot satisfy this query: one use needs `none`, this
one needs `record<account>``. Wrong: `$auth` is a session/context param, not a
host param, and `= NONE` is a legitimate optional check.

Root cause: in a function body `$auth` is an **unbound host param**, so
`WHERE in = $auth` exports a `record<account>` constraint and `$auth = NONE`
exports a `none` constraint → `constrain_param` reports the two as
irreconcilable (`crates/workspace/src/analyzer/context.rs:183-194`).

**Fix (primary):** seed the session/context params
(`$auth`, `$token`, `$session`, `$access`, `$scope`) as **bound facts** with
kind `option<record>` (`Either[None, Record[]]`) in DEFINE FUNCTION bodies and
general statement/query analysis — generalizing what `permissions.rs:96`
already does only for the PERMISSIONS context. Because
`constrain_param` early-returns for any param with a local fact
(`context.rs:179-182`), seeding kills the spurious 6001, AND the existing
`narrow.rs` early-return NONE-guard narrowing then refines `$auth` to
`record<>` in the fall-through after `IF $auth = NONE THEN RETURN`, so
`in = $auth` type-checks. This matches the user's model: `$auth` starts
optional/any and control flow narrows it — the machinery in
`crates/workspace/src/analyzer/flow/narrow.rs` already does exactly this for
declared params (see its e2e tests); it just never sees `$auth` today.

Also: reconcile the overlapping context/protected param lists
(`CONTEXT_ONLY_PARAMS` in `analyzer/expression/mod.rs:85`, the protected set in
`flow/let_stmt.rs:15`, and the `permissions.rs` session set) so they don't
drift — the diagnostics audit flagged this too.

### C2. Equality comparisons should not export hard kind constraints
Secondary, but the deeper cause: a loose `x = $p` / `in = $p` comparison
exports a *hard* param kind requirement, yet SurrealDB comparisons are
kind-ordered (comparing across kinds is legal). A comparison position should
contribute at most a soft/informational domain hint for host-param inference,
never a hard constraint that can conflict (6001) or a value-mismatch (2001).
Verify genuine host params compared with `=`/`IN` don't false-positive after
C1; tighten the constraint-export site if they do.

### C3. New opt-in lint: disallow `any` (user request)
Add a new `allow`-by-default lint (follows the rustc default-level model just
landed) that flags any inferred/declared `Kind::Any` reaching a
result/param/field position, so a team can opt in to force explicit casts.
Register it in `crates/diagnostics/src/catalog.rs` with `default_level = Allow`
and add a row to `docs/plans/2026-07-07-diagnostic-catalog.md` (the
registry↔doc consistency test enforces this). Pick the next free 7xxx number
(7016). Emit where a position resolves to `Any` that the user could annotate.

### C4. Casting correctness
The user asked to "make sure we properly handle casting" — the disallow-`any`
lint is only useful if `<record<x>> $auth` / `<int> $x` casts actually refine
the type. Audit the cast paths (2007/2008 + the cast inference) so a cast to a
concrete kind narrows the inferred kind (and clears the disallow-`any` lint),
and an impossible cast still reports 2008.

### C5. LIVE SELECT projection + param inference
`analyze_live_select` (`crates/workspace/src/analyzer/data/live_select.rs`)
doesn't compute the projection row type or param usage — a
`LIVE SELECT name, age FROM user WHERE team = $team` codegens as
`result: string` with no params. This is why `@surrealguard/svelte`'s
`liveQuery` can't be typed (Workstream A #10 depends on this). Extend the
LiveSelect AST + lowering to retain projection/WHERE (the diagnostics audit's
4009 note confirms lowering currently discards all clauses), then infer the row
type the same way `select.rs` does. Feeds the codegen registry entry.

**Note:** this workstream edits `crates/workspace/src/analyzer/**` and
`crates/diagnostics/src/catalog.rs`, which overlap the just-completed (and now
committed) default-lint-level work — it must build ON TOP of that commit, not a
pre-change base.

---

## Workstream D — LSP hover polish (gated)
See `docs/plans/2026-07-24-lsp-hover-polish.md`. Blocked behind the in-flight
hover reformat (`backend.rs` + `query.rs`); do after it lands. Items: syntax-
highlight the RHS kind, capitalize `Fields`, show simple permissions. Reminder:
the inline diagnostic `message` is plain text per LSP spec — rich formatting
belongs in the hover, not `crates/lsp/src/diagnostics.rs`.

---

## Execution partition (conflict-free)

Three file-disjoint fix workstreams run in parallel worktrees, each with its
own `CARGO_TARGET_DIR`, none touching `query.rs`/`backend.rs`:

| Agent | Owns (edits) | Depends on |
|---|---|---|
| A — TS/CLI | `crates/embed`, `crates/cli`, `packages/**`, `examples/surrealguard.toml`, TS docs | — |
| B — Rust adapter | `crates/macros`, `crates/rs`, `docs/plans/2026-07-15-rust-adapter.md` | — |
| C — Analyzer | `crates/workspace/src/analyzer/**`, `crates/diagnostics/src/catalog.rs`, `docs/plans/2026-07-07-diagnostic-catalog.md`, workspace tests | the committed default-lint-level base |

D (hover) is sequenced after the other agent's hover reformat lands. Each agent
runs its crate's tests + relevant e2e before reporting; integration merges are
clean because the file sets are disjoint.
