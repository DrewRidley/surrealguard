# SurrealGuard examples

Real, type-checked demos of the round-trip:

**schema (`.surql`) → `surrealguard generate` → typed queries.**

`surrealguard generate` scans your host files (`.ts`, `.tsx`, `.svelte`, …) for
query text, analyzes each query against your `schema/*.surql`, and writes
`surrealguard.generated.ts` — a module augmentation that keys every query by its
exact text with its `{ result; params }` types. You import the entry points
**from that generated file**, so the augmentation loads with them and everything
is typed: the result rows, and the params argument (required exactly when the
query reads params).

## Examples

| Example | What it shows |
| --- | --- |
| [`basic/`](./basic) | Vanilla TypeScript: `createClient` + `defineQuery` + `db.run` / `db.watch`. |
| [`sveltekit/`](./sveltekit) | Svelte 5 / SvelteKit: `preload` in `load`, `createLive` in the component, a shared query in a `.svelte.ts` module. |

Both type-check as part of `pnpm -r run typecheck`.

## A query is a value

Query text lives in exactly one file:

```ts
// src/queries.ts
import { defineQuery, defineLive } from "../surrealguard.generated";

export const allPeople  = defineQuery("SELECT id, name, age, team FROM person");
export const peopleOf   = defineQuery("SELECT id, name FROM person WHERE team = $team");
export const livePeople = defineLive("SELECT id, name, age, team FROM person");
```

Everything else imports the *value*. That is what stops an SSR seed and the
component that consumes it from drifting apart byte-for-byte and silently
missing the cache — the failure the SvelteKit example used to have built in,
with the same `SELECT` written out in both `+page.ts` and `+page.svelte`.

## Run the round-trip

From the repo root (the workspace links `@surrealguard/*`):

```sh
pnpm install

# 1. Build the CLI.
CARGO_TARGET_DIR=/tmp/sg cargo build --release -p surrealguard

# 2. Generate the typed registry from the schema + the project's queries.
cd examples/basic
/tmp/sg/release/surrealguard generate --out surrealguard.generated.ts

# 3. Type-check — the generated types make everything typed.
cd ../..
pnpm --filter @surrealguard-example/basic run typecheck
```

> **Known gap.** `crates/embed` recognises `` surql`…` ``, `surql("…")` and any
> `.query(…)` / `.live(…)` member call, but **not** `defineQuery("…")` /
> `defineLive("…")`. Step 2 therefore extracts nothing from these examples
> today, and their committed `surrealguard.generated.ts` files were produced by
> running the real analyzer over the same query texts through a recognised sink.
> Teaching the extractor the two new names is a ~3-line change to
> `is_surql_tag`, or a `[codegen] sinks` config key.

## The guarantee

The generated types are load-bearing, not decorative. Each example has a
`guarantees.ts` where every line is a compile error under `@ts-expect-error`:

```ts
// @ts-expect-error a plain string is not a RecordId<"team">. This used to
// typecheck and then match nothing on the wire.
await db.run(peopleOf, { team: "team:red" });

// @ts-expect-error `nope` is not in the generated result shape.
(await db.run(allPeople))[0]!.nope;
```

Delete a directive and the typecheck fails — `tsc` errors on an *unused*
`@ts-expect-error`, so these cannot rot into no-ops.

That `RecordId` line is worth dwelling on. A `RecordId` parameter encodes to a
record link on the wire (CBOR tag 8); a plain string encodes to a SurrealQL
string. Before 0.5.0 the generated type was a branded string, so
`WHERE team = $team` compared a `record<team>` against a string and matched
nothing — and the compiler was happy. Both examples now construct the parameter
properly:

```ts
import { RecordId } from "../surrealguard.generated";
await db.run(peopleOf, { team: new RecordId("team", "red") });
```

## Type-checking

`examples/basic` runs `tsc`. `examples/sveltekit` runs `svelte-check`, so its
`.svelte` components are checked too — including that `people.data` really is
`Array<{ id: \`person:${string}\`; name: string; age: number }>` inside the
`{#each}`.
