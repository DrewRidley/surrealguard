<div align="center">

# 🛡️ SurrealGuard

### Static analysis and type inference for SurrealQL

Catch unknown fields, kind mismatches, and bad graph traversals *before* a query
reaches SurrealDB — and get fully typed results in **TypeScript** and **Rust**.

[![crates.io](https://img.shields.io/crates/v/surrealguard?label=surrealguard&color=e07b39&logo=rust&logoColor=white)](https://crates.io/crates/surrealguard)
[![npm](https://img.shields.io/npm/v/@surrealguard/client?label=%40surrealguard%2Fclient&color=cb3837&logo=npm&logoColor=white)](https://www.npmjs.com/package/@surrealguard/client)
[![CI](https://github.com/DrewRidley/surrealguard/actions/workflows/ci.yml/badge.svg?branch=master)](https://github.com/DrewRidley/surrealguard/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-3b82f6)](#license)

[**Docs**](https://surrealguard.dev/docs/) · [**Live playground**](https://surrealguard.dev/#playground) · [**DESIGN.md**](docs/DESIGN.md)

</div>

---

## What it is

SurrealGuard parses your `.surql` schema and queries into a typed, span-carrying
AST, infers the response type of every statement, and reports violations of each
construct's contract. One engine, four front ends:

| | |
| --- | --- |
| **CLI** — `surrealguard` | `check` your workspace in CI, `generate` TypeScript types, `watch` both while you develop |
| **Language server** — `surrealguard-lsp` | Diagnostics, hover, inlay hints, go-to-definition, and type-aware completion, in `.surql` files *and* in SurrealQL embedded in TypeScript / Svelte / Vue / Astro |
| **TypeScript** — `@surrealguard/{client,query,next,svelte}` | `db.query("SELECT …")` typed from the query text; live queries as framework-native reactive state |
| **Rust** — `surrealguard-rs` | `query!("SELECT …")` checked and typed at compile time |

## Why

SurrealDB is permissive at runtime: cross-kind comparisons order by kind instead
of failing, coercions succeed silently, and a misspelled field just returns
`NONE`. That is exactly where bugs hide. SurrealGuard is **contract-first** —
every construct has a contract (what the author must mean for the statement to
make sense), and the analyzer reports violations of that contract even when the
engine would happily execute the query.

SurrealDB's own parser discards spans before producing its AST, so it cannot
power an analyzer or editor tooling. SurrealGuard parses with tree-sitter into a
typed, span-carrying AST and runs all analysis on that.

## Quickstart (TypeScript)

```sh
npm i @surrealguard/client surrealdb
npm i -D surrealguard typescript
npx surrealguard init          # writes a commented surrealguard.toml
```

Point `surrealguard.toml`'s `schema` glob at your `.surql` files, and write a
schema:

```surql
-- schema/schema.surql
DEFINE TABLE team SCHEMAFULL;
DEFINE FIELD name ON team TYPE string;

DEFINE TABLE person SCHEMAFULL;
DEFINE FIELD name ON person TYPE string;
DEFINE FIELD age ON person TYPE int;
DEFINE FIELD team ON person TYPE record<team>;
```

Write queries as ordinary string literals in your own code — those calls are
what `generate` reads. There is no separate query manifest, and nothing to wrap
the string in:

```ts
// src/main.ts
import { createClient, RecordId } from "./surrealguard.generated";

const db = createClient({
  url: "ws://localhost:8000/rpc",
  namespace: "app",
  database: "app",
}); // connects lazily; import `db` anywhere, provide it to nothing

const [people] = await db.query("SELECT name, age FROM person WHERE team = $team", {
  team: new RecordId("team", "red"),
});

for (const person of people) {
  console.log(person.name, person.age);
}
```

```sh
npx surrealguard generate --out src/surrealguard.generated.ts
```

```
generated src/surrealguard.generated.ts (1 query, 8ms)
```

While you are developing, run it as a loop instead — `watch` checks the whole
workspace on every save and regenerates when the check passes, so the types
never go stale behind you:

```sh
npx surrealguard watch --out src/surrealguard.generated.ts
```

`generate` scanned `src/main.ts`, analyzed the query against the schema, and
wrote a module that re-exports the client together with a registry keyed by the
query's exact text:

```ts
declare module "@surrealguard/client" {
  interface SurqlRegistry {
    "SELECT name, age FROM person WHERE team = $team": {
      result: [Array<{ age: number; name: string }>];
      params: { team: RecordId<"team"> };
    };
  }
}
```

Importing `createClient` **from that generated file** is what loads the
registry. `people` is now `Array<{ name: string; age: number }>`, the `team`
param is required and must be a `RecordId<"team">`, and `person.nope` is a
compile error. `tsc --noEmit` proves it.

A query built at runtime is not in the registry and resolves to `unknown[]` — it
still runs; there is no `any` in the API. The client composes the official
`surrealdb` SDK rather than extending it, so the raw `Surreal` instance stays
reachable at `db.surreal`.

Three things are worth stating because each one produces `any` **with no error
on your own code**:

- `@surrealguard/client` must actually be installed. The generated file says
  `declare module "@surrealguard/client"`; if that specifier does not resolve,
  TypeScript reports `TS2664` *inside the generated file* and silently drops the
  whole registry.
- `--out` must match how you import it. Bare `generate` writes to the workspace
  root, which is usually not where `./surrealguard.generated`,
  `$lib/surrealguard.generated` or `@/surrealguard.generated` points. Pin it in
  `package.json` once.
- A `.svelte` `<script>` needs `lang="ts"`. Without it Svelte does not typecheck
  the block at all.

See [`@surrealguard/client`](packages/client) for the full contract — including
[when everything is `any`](packages/client/README.md#when-everything-is-any) —
and [`examples/`](examples) for a vanilla-TS and a SvelteKit project you can run.

## Check in CI

```sh
npx surrealguard check
```

`check` analyzes both your `.surql` files and the SurrealQL embedded in host
files, and reports findings rustc-style at their real `file:line`:

```
error[E1002]: `person` has no field `ag`
  --> src/routes/+page.svelte:6:39
  |
6 |   const rows = db.query("SELECT name, ag FROM person");
  |                                       ^^
  |
  = help: did you mean `age`?
note: `person` is defined here
  --> schema/schema.surql:4:14
  |
4 | DEFINE TABLE person SCHEMAFULL;
  |              ^^^^^^

checked 3 sources in 6ms
found 1 error
```

On a terminal the severity, code, paths and carets are coloured; piped to a file
or a CI log it is the same text with no escape sequences, and `--no-color` /
`NO_COLOR` turn colour off explicitly.

The exit code reflects the post-policy error count, so it drops straight into
CI. `--json` emits `{ summary, diagnostics[] }` with byte-offset ranges for
tooling — one document, one exit code, never decorated. `generate` runs the same
analysis and refuses to write a registry when an embedded query has an error, so
a broken build can never overwrite good types.

## Live queries, typed

`db.query` covers reading. When you want rows that *keep* updating, name the
query once and subscribe to it. Vanilla TypeScript needs no extra package:

```ts
import { defineLive } from "./surrealguard.generated";

const livePeople = defineLive("SELECT id, name, age FROM person");
const stop = db.watch(livePeople, (rows) => render(rows));
//    ^? rows: Array<{ id: RecordId<"person">; name: string; age: number }>
```

The framework adapters turn the same reference into framework-native reactive
state, on a shared core (`@surrealguard/query`) that owns the cache,
reference-counts subscriptions, and reconciles `LIVE SELECT` notifications by
record id.

```svelte
<script lang="ts">
  import { createLive } from "@surrealguard/svelte";
  import { livePeople } from "$lib/queries";

  // runes-reactive — read it directly, no store `$` prefix
  const people = createLive(livePeople);
</script>

{#each people.data as person (person.id)}<li>{person.name}</li>{/each}
```

`@surrealguard/next` offers the same as a hook: `const people =
useLive(livePeople)`. Both packages seed from the server with `preload(db,
livePeople)`, whose payload carries its own cache key — so the component
subscribes to exactly the query the server ran without naming it twice, the
first paint has no loading gap, and it upgrades to live in place.

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
It resolves the schema from `SURREALGUARD_SCHEMA`, or from a `schema/` or
`migrations/` directory under the crate root (applied in filename order).
`surql!` is the lighter form: check a query, expand to its text.

## Editors

`surrealguard-lsp` serves the analyzer over stdio for `.surql` files and for
SurrealQL embedded in host files, so squiggles land on the exact token inside an
inline query. It provides diagnostics, **type-aware completion** (fields, tables,
`$params`, `fn::` and builtin paths, graph steps that only offer traversable
edges), **inlay type hints** on `LET` bindings, **hover** on tables, `LET`
variables and context params, and go-to-definition. It reads `surrealguard.toml`,
so your `[lints]` levels apply live in the editor.

Zed users can install the
[`DrewRidley/zed-surreal`](https://github.com/DrewRidley/zed-surreal) extension;
any other LSP-capable editor can point at the binary directly.

For TypeScript and JavaScript files there is a second, lighter option:
[`@surrealguard/ts-plugin`](packages/ts-plugin), a TypeScript **language service
plugin**. One entry in `tsconfig.json` —

```jsonc
{ "compilerOptions": { "plugins": [{ "name": "@surrealguard/ts-plugin" }] } }
```

— and the findings inside your `db.query("…")` strings come back as
TypeScript's own diagnostics, with TypeScript's own classifications on the
query's tokens. That matters beyond convenience: the standalone LSP is a
*second* server answering about the same bytes as TypeScript, and the editor
resolves that competition differently on every keystroke, so an inline query
flickers between highlighted and plain string. A plugin has nothing to race —
its answers *are* TypeScript's. It does not load in `tsc` (that is TypeScript's
design), which is the right split: CI keeps running `surrealguard check`, which
sees the whole workspace instead of one file at a time. `.svelte` and `.vue`
still need the LSP; the tools that own those files build their TypeScript
service directly and never read `compilerOptions.plugins`.

## What it analyzes

- **Full statement coverage** — SELECT (projections, graph traversals,
  FETCH/SPLIT/GROUP/OMIT), the six mutations, RELATE, LET/RETURN/IF/FOR/blocks,
  transactions, DEFINE/REMOVE/ALTER, LIVE SELECT/KILL, and the rest.
- **Type inference** — response kinds as upstream `surrealdb_types::Kind`: closed
  object literals for known rows, unions from IF/ELSE, record-link and graph-edge
  shapes, the full builtin function table plus `fn::` declarations, closure and
  subquery inference, and constant-value evaluation.
- **A contract catalog of 87 diagnostics** in families (1xxx schema references,
  2xxx types, 3xxx graph, 4xxx statement misuse, 5xxx functions, 6xxx parameters,
  7xxx lints, 8xxx version compatibility). Severities are intrinsic to each
  finding; consumers apply policy (warnings-as-errors, lint levels) at their edge,
  rustc-style. Messages lead with the consequence and attach `help:` fixes and
  `note:` spans pointing at the relevant definition.
- **Flow-sensitive checks** — control-flow type narrowing (occurrence typing that
  narrows `option<T>` and `record<A|B>` unions via `= NONE` guards and
  `type::table()` discriminants, field paths included), PERMISSIONS-predicate
  analysis, comparison footguns (`= NONE`/`= NULL` on a kind that excludes it,
  `IN`/`CONTAINS` element-kind mismatch, disjoint record-link equality), aggregate
  `count()` without `GROUP`, `record<UndefinedTable>`, `DEFAULT` violating a
  field's own `ASSERT`, and unreachable code.
- **Parameter constraints** — every `$param` a source reads is exported with the
  kind and value domain its uses imply (`UPDATE user SET age = $age` → `age: int`;
  `LIMIT $n` → non-negative int).
- **Byte-precise spans** on every finding, for editor squiggles.

## Configuration

`surrealguard.toml` is discovered by walking up from the working directory; the
directory holding it is the workspace root. `surrealguard init` writes a fully
commented starter.

```toml
[sources]
schema = ["schema/**/*.surql"]     # DEFINE/REMOVE catalog; analyzed first
queries = ["queries/**/*.surql"]   # analyzed against that schema
ignore = ["node_modules/**", "target/**"]

[analysis]
strict = false

[diagnostics]
warnings_as_errors = false

# "allow" | "warn" | "deny". A specific code beats a family wildcard.
[lints]
# E1002 = "allow"
# "7xxx" = "warn"
```

Host files (`.ts`, `.tsx`, `.js`, `.jsx`, `.svelte`, `.vue`, `.astro`) are
scanned for embedded queries under the same `ignore` patterns — they need no
glob of their own.

## Install

**TypeScript / CLI:**

```sh
npm i @surrealguard/client surrealdb    # + @surrealguard/{query,next,svelte}
npm i -D surrealguard                   # the CLI, as a project dev dependency
```

The `surrealguard` npm package is a launcher that downloads the prebuilt binary
matching its version. `npx surrealguard --help` works without installing.

**Rust:**

```sh
cargo add surrealguard-rs        # the query! / surql! macros
cargo binstall surrealguard      # the CLI, prebuilt; `cargo install surrealguard` builds it
cargo install surrealguard-lsp   # the language server
```

Prebuilt archives for both binaries, every supported target, are attached to each
[GitHub Release](https://github.com/DrewRidley/surrealguard/releases).

## Project layout

```
surrealguard/
├── crates/
│   ├── syntax/                # tree-sitter parsing, typed span-carrying AST, lowering
│   ├── tree-sitter-surrealql/ # the vendored SurrealQL grammar
│   ├── diagnostics/           # finding types, code catalog, severity/lint policy
│   ├── workspace/             # schema index, analyzers, inference, analysis pipeline
│   ├── codegen/               # Kind → TypeScript generation
│   ├── embed/                 # embedded-SurrealQL extraction from host files
│   ├── macros/                # the surql! / query! proc-macros
│   ├── rs/                    # surrealguard-rs runtime (typed results)
│   ├── cli/                   # the `surrealguard` binary
│   ├── lsp/                   # the `surrealguard-lsp` binary
│   └── wasm/                  # the browser-playground analyzer build
├── packages/          # @surrealguard/{client,query,next,svelte} (pnpm workspace)
├── examples/          # runnable vanilla-TS and SvelteKit projects
└── docs/              # DESIGN.md + design plans (incl. the diagnostic catalog)
```

The grammar is vendored in-repo at `crates/tree-sitter-surrealql`, copied
verbatim from the official
[`surrealdb/surrealql-tree-sitter`](https://github.com/surrealdb/surrealql-tree-sitter)
(SurrealGuard's precedence fix and feature additions were merged there upstream).

## Contributing

`cargo test --workspace` runs the Rust suite; `cargo clippy --workspace
--all-targets` with `RUSTFLAGS="-D warnings"` is the CI gate. For the TypeScript
packages: `pnpm -r run build && pnpm -r run typecheck && pnpm -r --if-present run
test`. [`AGENTS.md`](AGENTS.md) documents the precision harnesses (snapshot,
`any` ratchet, real-binary LSP tests) that guard against silent regressions, and
[`docs/DESIGN.md`](docs/DESIGN.md) covers architecture and roadmap.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state otherwise,
any contribution intentionally submitted for inclusion in this project by you, as
defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
