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
   lints. The `range` is a byte offset into `source`
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
  - `crates/workspace/tests/narrowing_floor.rs` — an **upper bound** on every
    corpus site's type: what the hand-written narrowing recognizers inferred on
    the commit before they were deleted. Inference may infer anything narrower
    and does; it may not infer anything wider, and may not lose a site that has
    a type. Unlike the precision snapshot this is an inequality, so it does not
    move when precision improves — which is what makes it still catch a widening
    after someone has regenerated the snapshot to accept one. **Do not
    regenerate it to make it pass**: it records a path that no longer exists, so
    rewriting it from the current path turns the check into a tautology.
  - `crates/syntax/tests/conformance.rs` — the **grammar-conformance
    gate**, two corpora extracted from SurrealDB's own test suites and both
    held at 100%. `crates/syntax/examples/conformance_corpus.json` is the
    **valid** set (a JSON array of queries): a parse error is fatal to the
    whole source, so every entry the grammar rejects is a place the analyzer
    is silently wrong — fix the grammar, never the corpus.
    `crates/syntax/examples/conformance_rejected.json` is the **rejected**
    set (`[query, reason]` pairs): text from the same suites that is not
    SurrealQL (deliberate parser-error fragments, regex assertions on `INFO`
    output), every entry of which must *fail* to parse — one that starts
    parsing is over-acceptance. There is no baseline file and no
    `UPDATE_SNAPSHOTS` path: the invariant is absolute, and moving an entry
    between the sets is a deliberate edit with a reason. The human-readable
    report is `cargo run -p surrealguard-syntax --example conformance`, and
    `docs/grammar-conformance.md` records the history. A handful of forms
    are parsed on purpose despite the engine refusing them, so the analyzer
    can diagnose them precisely instead of the file collapsing into a syntax
    error; `crates/workspace/tests/engine_refused_syntax.rs` pins that each
    one raises its contract code. The grammar
    itself is `crates/tree-sitter-surrealql/grammar.js`; after editing it,
    regenerate with `npx --yes tree-sitter-cli@0.25.10 generate --abi 14` in
    that directory (the `tree-sitter` CLI is not a workspace dependency).
  - `crates/syntax/tests/robustness.rs` — the **front-end robustness
    property**. The LSP runs `parse_source` + `lower_statements` on every
    keystroke, so their input is mostly half-typed: `proptest` generates
    random bytes, corpus statements (the conformance corpus plus
    `crates/workspace/tests/corpus/**/*.surql`) with single-character
    deletions/insertions/truncations, and concatenations of those, and asserts
    nothing panics, every span in the lowered AST (walked exhaustively by
    `tests/support/ast_walk.rs`), every syntax diagnostic and every highlight
    token is in bounds and on a UTF-8 boundary, and a recovery `Partial`
    (`ERROR`/`MISSING …`) appears only when the CST has an error. 256 cases
    by default (`PROPTEST_CASES=<n>` raises it; a failing input is saved under
    `crates/syntax/proptest-regressions/`). Beside it, `tests/recovery.rs`
    pins the recovery shape of each common half-typed input (which statement
    goes `Partial`, where the diagnostic points, that the neighbours keep
    their spans) and `tests/multibyte.rs` pins spans over `é`/emoji/CJK text;
    the byte → UTF-16 position conversion itself lives in
    `crates/lsp/src/text.rs`.
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
  - `crates/codegen/tests/golden.rs` — the **generated-TypeScript golden**.
    `surrealguard generate` emits a module nothing used to compile, so a type
    error in the emitter's output would ship undetected. The test runs the
    CLI's generation path (`QueryEntry::from_analysis` + `render_registry`)
    over the fixture workspace `crates/codegen/tests/fixtures/typecheck/`
    (schema with option/record/array/literal-union/object fields, an edge
    table, `fn::` functions, a host `src/queries.ts`) and compares the module
    byte-for-byte with `packages/client/test-d/gen/surrealguard.generated.ts`.
    That file is then compiled by `pnpm -r run typecheck` as part of
    `@surrealguard/client` against the real `surrealdb` types, and
    `test-d/gen/*.test-d.ts` + `test/generated.test.ts` assert what the
    resolved types are. Regenerate with
    `UPDATE_SNAPSHOTS=1 cargo test -p surrealguard-codegen --test golden`, then
    run the package typecheck — the golden records *current* output, and the
    Rust side cannot tell whether it is valid TypeScript. Augment
    `SurqlRegistry` only through `"@surrealguard/client"` in that package
    (never `../src/registry.js`): one interface augmented through two
    specifiers gets two merged clones, and which one a file sees depends on
    program order.
- **Grammar:** the parser is the vendored `crates/tree-sitter-surrealql`
  (grammar.js plus its generated parser); see the conformance-ratchet entry
  above for how to edit and regenerate it.
- **Design principle — contract-first diagnostics:** every construct has a
  contract; severity derives from the contract violation, never from engine
  tolerance. One code per contract. Don't add denylists or permutation codes.
- Don't commit, push, or publish unless explicitly asked.

## Layout

- `crates/syntax` — tree-sitter parsing + typed span-carrying AST
- `crates/workspace` — schema index, analyzers, inference (the engine).
  Whole-pipeline tests live in `crates/workspace/tests/pipeline/` (one file
  per area, shared helpers in `tests/support/mod.rs`); per-contract suites in
  `tests/{select,mutation,ddl,version}_contracts.rs` and `tests/contract_guards.rs`
  (a fire + near-miss pair for every Deny code). Do not add tests to
  `src/analysis.rs`.
- `crates/diagnostics` — finding codes, severities, policy. `catalog.rs` is the
  single source of truth for the code list; the published catalog page
  (`web/public/docs/diagnostics.html`) is **generated** from it — add a code,
  then run `pnpm docs:diagnostics`. CI fails if the page is stale.
- `crates/macros` + `crates/rs` — the `query!` / `surql!` macros and runtime
- `crates/codegen` + `crates/embed` — TypeScript generation + host-file extraction
- `crates/cli` + `crates/lsp` — the `surrealguard` and `surrealguard-lsp` binaries
- `packages/` — `@surrealguard/{client,query,next,svelte}`
- `docs/DESIGN.md` — architecture; `docs/plans/` — design records
