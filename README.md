<div align="center">

# 🛡️ SurrealGuard

### Static analysis &amp; type inference for SurrealQL

Catch unknown fields, kind mismatches, and bad graph traversals *before* a query
reaches SurrealDB — with fully typed results in **Rust** and **TypeScript**.

[![npm](https://img.shields.io/npm/v/@surrealguard/client?label=%40surrealguard%2Fclient&color=cb3837&logo=npm&logoColor=white)](https://www.npmjs.com/package/@surrealguard/client)
[![CI](https://github.com/DrewRidley/surrealguard/actions/workflows/ci.yml/badge.svg?branch=master)](https://github.com/DrewRidley/surrealguard/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-3b82f6)](#license)

[**Docs**](https://surrealguard.dev/docs/) · [**Live playground**](https://surrealguard.dev/#playground) · [**npm**](https://www.npmjs.com/org/surrealguard) · [**DESIGN.md**](docs/DESIGN.md)

</div>

---

SurrealGuard parses your `.surql` schema and queries into a typed, span-carrying
AST, infers the response type of every statement, and reports violations of each
construct's contract. That same engine powers a CLI, a language server, a Rust
proc-macro, and a set of TypeScript packages.

## Why

SurrealDB is permissive at runtime: cross-kind comparisons order by kind instead
of failing, coercions succeed silently, and a misspelled field just returns
`NONE`. That's exactly where bugs hide. SurrealGuard is **contract-first** —
every construct has a contract (what the author must mean for the statement to
make sense), and the analyzer reports violations of that contract even when the
engine would happily execute the query.

SurrealDB's own parser discards spans before producing its AST, so it can't power
an analyzer or editor tooling. SurrealGuard parses with tree-sitter into a typed,
span-carrying AST and runs all analysis on that.

## The compiler is the checker (Rust)

```rust
use surrealguard_rs::query;

// Checked against your schema at compile time. A wrong table, unknown field,
// bad arity, or kind mismatch is a `cargo check` error — no external step.
let users = query!("SELECT name, age FROM user");
//  users: Query<Vec<{ name: String, age: i64 }>>  ← nameless, inferred
```

`query!` runs the real analyzer during compilation and turns findings into
spanned `compile_error!`s, then generates the result type from the inferred
response kind — no codegen step, no language server, no runtime schema fetch.
Point it at a `schema/` or `migrations/` directory (applied in order) and it
resolves real tables and fields. `surql!` is the lighter form: check a query,
expand to its text.

## Typed queries, no wrapper (TypeScript)

```ts
// One import: the generated file re-exports a ready client that IS a SurrealDB
// `Surreal` (every SDK method) and loads the typed query registry.
import { SurrealGuardClient } from "./surrealguard.generated"; // from `surrealguard generate`

const db = new SurrealGuardClient();
await db.connect("ws://localhost:8000/rpc");

// SurrealDB returns one result per statement, so destructure the first result.
const [users] = await db.query("SELECT name FROM user WHERE team = $team", { team: "red" });
//     ^ result typed from the query text; params required + typed; wrong/missing params
//       are compile errors. Dynamic strings degrade to `unknown[]` and still run.
```

Framework adapters build on a reactive core (`@surrealguard/query`) that hides
SSR hydration and live-query lifetimes:

```svelte
<script>
  import { liveQuery } from "@surrealguard/svelte";
  // `db` comes from Svelte context (setClient in +layout.svelte); `users` is
  // runes-reactive — read it directly, no store `$` prefix.
  const users = liveQuery((db) => db.live(`SELECT * FROM user`));
</script>
{#each users.data as user (user.id)}<li>{user.name}</li>{/each}
```

`db.live(...)` opens one shared, reference-counted `LIVE SELECT` subscription and
reconciles change notifications by record id, with the row type inferred just
like `db.query`. `@surrealguard/next` offers the same via a `useLiveQuery` hook
(`const { data } = useLiveQuery((db) => db.live(`…`))`).

## What it analyzes

- **Full statement coverage** — SELECT (projections, graph traversals,
  FETCH/SPLIT/GROUP/OMIT), the six mutations, RELATE, LET/RETURN/IF/FOR/blocks,
  transactions, DEFINE/REMOVE/ALTER, LIVE SELECT/KILL, and the rest.
- **Type inference** — response kinds as upstream `surrealdb_types::Kind`: closed
  object literals for known rows, unions from IF/ELSE, record-link and graph-edge
  shapes, the full builtin function table plus `fn::` declarations, closure and
  subquery inference, and constant-value evaluation.
- **A contract catalog of ~80 diagnostics** in families (1xxx schema references,
  2xxx types, 3xxx graph, 4xxx statement misuse, 5xxx functions, 6xxx parameters,
  7xxx lints, 8xxx version compatibility). Severities are intrinsic to each
  finding; consumers apply policy (warnings-as-errors, lint levels) at their edge,
  rustc-style.
- **Parameter constraints** — every `$param` a source reads is exported with the
  kind and value domain its uses imply (`UPDATE user SET age = $age` → `age: int`;
  `LIMIT $n` → non-negative int).
- **Byte-precise spans** on every finding, for editor squiggles.

## Ways to use it

- **CLI** — `surrealguard check` analyzes a workspace (`--json` for machine
  output; exit code reflects post-policy errors); `surrealguard generate` emits
  the TypeScript types; `surrealguard init` writes a starter config.
- **Editors (LSP)** — `surrealguard-lsp` publishes diagnostics over stdio for
  `.surql` files *and* for SurrealQL embedded in host files (TypeScript, Svelte,
  Vue, Astro) — squiggles land on the exact token inside your inline query.
- **Rust** — the `surrealguard-rs` crate re-exports the `query!` / `surql!` macros.
- **TypeScript** — `@surrealguard/client`, `@surrealguard/query`,
  `@surrealguard/next`, `@surrealguard/svelte`.

## Project layout

```
surrealguard/
├── crates/
│   ├── syntax/        # tree-sitter parsing, typed span-carrying AST, lowering
│   ├── diagnostics/   # finding types, code catalog, severity/lint policy
│   ├── workspace/     # schema index, analyzers, inference, analysis pipeline
│   ├── codegen/       # Kind → TypeScript generation
│   ├── embed/         # embedded-SurrealQL extraction from host files
│   ├── macros/        # the surql! / query! proc-macros
│   ├── rs/            # surrealguard-rs runtime (typed results)
│   ├── cli/           # the `surrealguard` binary
│   └── lsp/           # the `surrealguard-lsp` binary
├── packages/          # @surrealguard/{client,query,next,svelte} (pnpm workspace)
└── docs/              # DESIGN.md + design plans (incl. the diagnostic catalog)
```

## Install

**TypeScript** (published):

```bash
npm i @surrealguard/client      # + @surrealguard/{query,next,svelte}
```

**Rust / CLI** (from source while the crates.io release is finalized):

```bash
cargo install --path crates/cli
surrealguard init && surrealguard check
```

The workspace analyzes every `.surql` source in order, so schema definitions are
visible to the queries that follow them.

## Status

The engine (typed AST, full inference, ~80 contract diagnostics, parameter
constraints), the CLI, the LSP, the Rust `surql!` / `query!` macros, and the
TypeScript packages are all built and tested with CI green.

- **npm** — `@surrealguard/{client,query,next,svelte}` are **published** (0.2.x).
- **Grammar** — the precedence fix and feature additions are **merged upstream**
  into [`surrealdb/surrealql-tree-sitter`](https://github.com/surrealdb/surrealql-tree-sitter).
- **crates.io** — the Rust crates (`surrealguard`, `surrealguard-rs`) are next.

See [`docs/DESIGN.md`](docs/DESIGN.md) for architecture and roadmap.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state otherwise,
any contribution intentionally submitted for inclusion in this project by you, as
defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
