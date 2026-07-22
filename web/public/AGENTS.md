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
   - TypeScript: run `surrealguard generate` and pass string literals to
     `SurrealGuardClient.query("…")` from `@surrealguard/client`.

## Working inside this repository

- **Rust workspace** (`crates/`): `cargo test --workspace` runs the suite;
  `RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets` is the CI gate
  and must stay clean. `missing_docs` is enforced — every public item needs a doc
  comment. Format with `cargo fmt --all`.
- **TypeScript packages** (`packages/`, pnpm workspace): `pnpm -r run build`,
  `pnpm -r run typecheck`, `pnpm -r --if-present run test`.
- **Grammar:** the parser is `tree-sitter-surrealql`, a path dependency at the
  sibling `../tree-sitter-surrealql`. CI checks it out alongside this repo.
- **Design principle — contract-first diagnostics:** every construct has a
  contract; severity derives from the contract violation, never from engine
  tolerance. One code per contract. Don't add denylists or permutation codes.
- Don't commit, push, or publish unless explicitly asked.

## Layout

- `crates/syntax` — tree-sitter parsing + typed span-carrying AST
- `crates/workspace` — schema index, analyzers, inference (the engine)
- `crates/diagnostics` — finding codes, severities, policy
- `crates/macros` + `crates/rs` — the `query!` / `surql!` macros and runtime
- `crates/codegen` + `crates/embed` — TypeScript generation + host-file extraction
- `crates/cli` + `crates/lsp` — the `surrealguard` and `surrealguard-lsp` binaries
- `packages/` — `@surrealguard/{client,query,next,svelte}`
- `docs/DESIGN.md` — architecture; `docs/plans/` — design records
