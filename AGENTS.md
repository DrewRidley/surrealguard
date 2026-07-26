# AGENTS.md

Guidance for AI coding agents working with **SurrealGuard** — a static analyzer
and type-inference engine for SurrealQL. This file follows the
[agents.md](https://agents.md) convention. A machine-readable summary also lives
at [`/llms.txt`](https://surrealguard.dev/llms.txt) and
[`/llms-full.txt`](https://surrealguard.dev/llms-full.txt).

## Using SurrealGuard in a user's project

1. **Set it up:** `npx surrealguard init`, then edit `surrealguard.toml` so
   `[sources] schema` and `queries` globs point at the project's `.surql` files.
2. **Check on every change:** `npx surrealguard check --json`. The JSON is
   `{ summary, diagnostics[] }`; each diagnostic has `code`, `severity`
   (`error`/`warning`/`hint`), `source`, `range { start, end }` (byte offsets),
   `message`, and `help`. The process exit code is non-zero when errors remain
   after policy — use it as a CI/agent gate.
3. **Fix by code + span.** Codes are grouped: 1xxx schema references, 2xxx types,
   3xxx graph, 4xxx statement misuse, 5xxx functions, 6xxx parameters, 7xxx
   lints, 8xxx version compatibility. The `range` is a byte offset into `source`
   — apply edits there.
4. **Type the queries:**
   - Rust: wrap queries in the `query!` macro (`cargo add surrealguard-rs`). They
     are checked at compile time; a violation fails `cargo check`.
   - TypeScript: run `surrealguard generate --out src/surrealguard.generated.ts`,
     import `SurrealGuardClient` from that file (it extends the `surrealdb` SDK),
     and pass string literals to `db.query("…")` — destructure the first result,
     `const [rows] = await db.query("…")`.

## Working inside this repository

- **Rust workspace** (`crates/`): `cargo test --workspace` runs the suite;
  `RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets` is the CI gate
  and must stay clean. `missing_docs` is enforced — every public item needs a doc
  comment. Format with `cargo fmt --all`.
- **TypeScript packages** (`packages/`, pnpm workspace): `pnpm -r run build`,
  `pnpm -r run typecheck`, `pnpm -r --if-present run test`.
- **Quality harness — lost precision.** The unit suite proves nothing is newly
  *wrong*; it cannot see a type quietly degrading to `unknown`, a narrowing
  dying, or a completion disappearing. Three harnesses cover that, all driven by
  the committed corpus at `crates/workspace/tests/corpus/` (self-contained on
  purpose — the realistic corpus lives outside this repo and is hand-edited, so
  it can never back a committed snapshot):
  - `crates/workspace/tests/precision_snapshot.rs` — a golden file of **every**
    inferred type the corpus produces. Regenerate with
    `UPDATE_SNAPSHOTS=1 cargo test -p surrealguard-workspace --test precision_snapshot`.
    The snapshot records *current* behaviour, not correct behaviour: read every
    diff before accepting it.
  - `crates/workspace/tests/any_ratchet.rs` — a per-site `any`/`unknown` count
    held against a committed baseline. Precision may improve freely, never
    degrade. Failures list the *sites*, not a total. A genuinely unknowable site
    is frozen with a `# expected: <reason>` note in the baseline. Regenerate with
    `UPDATE_SNAPSHOTS=1 cargo test -p surrealguard-workspace --test any_ratchet`
    (regenerating preserves the `# expected:` notes).
  - `scripts/oracle.py` — the **real-world corpus gate**, run against the
    hand-edited workspace outside this repo (`../workshop/database`). It is a
    *triage* gate, not a count: `tests/oracle_baseline.txt` records every finding
    with a verdict, and the gate reports what is NEW (needs triage) and what is
    GONE (a check stopped firing — usually a regression). Run
    `scripts/oracle.py check`; after triaging, `scripts/oracle.py update`.

    **Do not treat the finding count as the invariant.** That corpus is not
    all-valid — it contains genuinely broken SurrealQL — so the count *should*
    move when a diagnostic is added, corrected, or a real bug is caught. Holding
    it flat actively suppresses correct work: gating aggregate promotion on a
    `GROUP` clause was once declined purely because it would add +2 findings,
    even though the engine rejects both of those queries outright. A bare count
    also hides the worst case — one gained plus one lost reads as no change.

  - `crates/lsp/tests/stdio.rs` — spawns the **real** `surrealguard-lsp` binary
    and asserts on hover, inlay hints, completion and diagnostics at specific
    cursor positions. `crates/lsp/tests/backend.rs` drives the service in-process
    and so cannot catch a surface that is wrong only over the wire. Requests must
    be sequenced (`initialize` → its response → `initialized` → `didOpen` →
    request) or tower-lsp answers "Server not initialized".
- **Grammar:** the parser is `tree-sitter-surrealql`, a path dependency at the
  sibling `../tree-sitter-surrealql`. CI checks it out alongside this repo.
- **Design principle — contract-first diagnostics:** every construct has a
  contract; severity derives from the contract violation, never from engine
  tolerance. One code per contract. Don't add denylists or permutation codes.
- Don't commit, push, or publish unless explicitly asked.

## Layout

- `crates/syntax` — tree-sitter parsing + typed span-carrying AST
- `crates/workspace` — schema index, analyzers, inference (the engine)
- `crates/diagnostics` — finding codes, severities, policy. `catalog.rs` is the
  single source of truth for the code list; the published catalog page
  (`web/public/docs/diagnostics.html`) is **generated** from it — add a code,
  then run `pnpm docs:diagnostics`. CI fails if the page is stale.
- `crates/macros` + `crates/rs` — the `query!` / `surql!` macros and runtime
- `crates/codegen` + `crates/embed` — TypeScript generation + host-file extraction
- `crates/cli` + `crates/lsp` — the `surrealguard` and `surrealguard-lsp` binaries
- `packages/` — `@surrealguard/{client,query,next,svelte}`
- `docs/DESIGN.md` — architecture; `docs/plans/` — design records
