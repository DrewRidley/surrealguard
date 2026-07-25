# crates.io release runbook — SurrealGuard 0.3.0

Ordered runbook for publishing the whole SurrealGuard tree to crates.io at
`0.3.0`, matching the already-published `@surrealguard/*` npm 0.3.0.

## Preconditions

- Branch `redesign-v3-foundation`, working tree reviewed and committed (the
  version bump, per-crate READMEs, and doc comments prepared for this release).
- `[workspace.package] version = "0.3.0"` in the root `Cargo.toml`; every
  internal path-dependency constraint reads `version = "0.3.0"`. The
  `surrealguard-wasm` crate stays `version = "0.1.0"` / `publish = false` and is
  **not** published.
- Logged in: `cargo login <token>` (a token with publish scope for the
  `DrewRidley`-owned crates). `surrealguard`, `surrealguard-macros`, etc. already
  exist on crates.io at 0.1.0 — 0.3.0 is a forward version bump, no name
  reservation needed.
- Verified locally (all green):
  - `CARGO_TARGET_DIR=/tmp/sg-release cargo build --release`
  - `CARGO_TARGET_DIR=/tmp/sg-release cargo test --doc -p surrealguard-rs -p surrealguard-macros -p surrealguard`
  - `cargo package -p surrealguard-tree-sitter-surrealql` and
    `cargo package -p surrealguard-embed` package **and verify** cleanly.

## Publish order (bottom-up)

Each crate must be **live on crates.io before the crate that depends on it** is
published, because `cargo publish` resolves the path-dep's `version = "0.3.0"`
constraint against the registry (not the local path). crates.io propagation is
usually a few seconds; modern `cargo` waits for the index and retries, so back-to-
back publishes normally just work. If one races the index, wait ~30s and re-run
that single step.

Run each from the repo root:

```bash
# 1. grammar — leaf, no SurrealGuard deps. MUST ship the vendored C sources.
cargo publish -p surrealguard-tree-sitter-surrealql

# 2. syntax — depends on the grammar (1).
cargo publish -p surrealguard-syntax

# 3. diagnostics — depends on syntax (2).
cargo publish -p surrealguard-diagnostics

# 4. workspace — depends on diagnostics (3) + syntax (2).
cargo publish -p surrealguard-workspace

# 5a. codegen — depends on workspace (4).
cargo publish -p surrealguard-codegen

# 5b. embed — leaf, no SurrealGuard deps; publishable any time after login.
cargo publish -p surrealguard-embed

# 6. macros — depends on workspace (4) + diagnostics (3).
cargo publish -p surrealguard-macros

# 7. rs — depends on macros (6). The flagship SDK crate.
cargo publish -p surrealguard-rs

# 8. cli (package name `surrealguard`) — depends on codegen (5a), diagnostics (3),
#    embed (5b), syntax (2), workspace (4).
cargo publish -p surrealguard

# 9. lsp — depends on diagnostics (3), embed (5b), syntax (2), workspace (4).
cargo publish -p surrealguard-lsp
```

That is the full set — 10 crates. `surrealguard-wasm` is intentionally omitted
(`publish = false`).

Tip: dry-run any step first with
`cargo publish -p <crate> --dry-run` (it performs the package + verify build
against the current registry state; it will fail for a crate whose dep isn't live
yet, which is expected until you reach it in order).

## Per-crate caveats

- **surrealguard-tree-sitter-surrealql** — the vendored grammar. The published
  archive MUST contain `src/parser.c`, `src/scanner.c`, and the
  `src/tree_sitter/*.h` headers, plus `grammar.js` and `LICENSE`; the `include`
  list in its `Cargo.toml` pins exactly those. Verified: `cargo package --list`
  shows `src/parser.c`, `src/scanner.c`, `src/tree_sitter/{alloc,array,parser}.h`.
  Its `build.rs` compiles the C sources with the `cc` crate at build time, so
  downstream users need a C compiler — standard for tree-sitter grammar crates.
  Licensed MIT (grammar upstream is `surrealdb/surrealql-tree-sitter`); the rest
  of the tree is `MIT OR Apache-2.0`.
- **surrealguard-embed** — no SurrealGuard deps (only `tree-sitter` +
  `tree-sitter-typescript`); packages and verifies standalone.
- **surrealguard-rs** — the front-page SDK. Re-exports the `query!`/`surql!`
  macros and carries the `Query<T>` / `RecordLink<T>` runtime types. Its
  doctests are `ignore`/`no_run`-free-of-DB so `cargo test --doc` stays green.
- **surrealguard (cli)** — binary crate; carries `[package.metadata.binstall]`
  so `cargo binstall surrealguard` pulls the prebuilt GitHub Release asset. The
  asset naming there MUST stay in lockstep with the npm launcher and
  `.github/workflows/release.yml` (`v{version}` tag,
  `surrealguard-{version}-{target}.{tgz,zip}`).
- **surrealguard-workspace / -syntax / -diagnostics / -codegen / -lsp** — public
  APIs are not yet stable; their READMEs say so. Fine to publish; consumers
  should pin exact versions.

## docs.rs expectations

- docs.rs builds each crate on publish and renders the `//!` crate docs as the
  front page. `surrealguard-rs` carries the richest page (macros, schema
  resolution, Kind→Rust table, execution note); the pipeline crates each carry a
  solid overview.
- The grammar crate builds C via `cc` in its `build.rs`; docs.rs provides a C
  toolchain, so its build succeeds.
- No crate needs a custom `[package.metadata.docs.rs]` block — there are no
  feature-gated docs or non-default targets.
- After publishing, spot-check <https://docs.rs/surrealguard-rs> and
  <https://docs.rs/surrealguard> rendered correctly.

## Post-publish

- Tag the release (`git tag v0.3.0 && git push --tags`) so the binstall asset
  URLs resolve and the GitHub Release workflow runs.
- Confirm `cargo install surrealguard` and `cargo add surrealguard-rs` both
  resolve 0.3.0 from a clean environment.
