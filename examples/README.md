# SurrealGuard examples

Real, type-checked demos of the round-trip:

**schema (`.surql`) → `surrealguard generate` → typed queries.**

`surrealguard generate` scans your host files (`.ts`, `.tsx`, `.svelte`, …) for
query text — `db.query("…")`, `defineQuery("…")`, `defineLive("…")` — analyzes
each against your `schema/*.surql`, and writes a module augmentation that keys
every query by its exact text with its `{ result; params }` types. You import
the entry points **from that generated file**, so the augmentation loads with
them and everything is typed: the result rows, and the params argument (required
exactly when the query reads params).

## Examples

| Example | What it shows |
| --- | --- |
| [`basic/`](./basic) | Vanilla TypeScript. Starts with a bare `db.query("…")`; then `defineQuery` + `db.run` / `db.watch` for what a named query buys. |
| [`sveltekit/`](./sveltekit) | Svelte 5 / SvelteKit. `/` is `db.query` in a component and nothing else; `/live` adds `preload` in `load` + `createLive` in the component, plus a shared query in a `.svelte.ts` module; `/components` renders the same data with `<Query>` / `<LiveQuery>`, one live subscription per row. |

Both type-check as part of `pnpm -r run typecheck`.

## Start with `db.query`

There is no wrapper to learn. The plain literal form is fully typed:

```ts
const [people] = await db.query("SELECT id, name, age, team FROM person");
//     ^? Array<{ age: number; id: RecordId<"person">; name: string; team: RecordId<"team"> }>
```

## Then `defineQuery`, when a name earns its keep

`defineQuery` reads the same registry through the same conditional generic, so
it adds no type safety. It adds a *value*, written in one file:

```ts
// src/queries.ts
import { defineQuery, defineLive } from "./surrealguard.generated";

export const allPeople  = defineQuery("SELECT id, name, age, team FROM person");
export const peopleOf   = defineQuery("SELECT id, name FROM person WHERE team = $team");
export const livePeople = defineLive("SELECT id, name, age, team FROM person");
```

Everything else imports the value. That is what stops an SSR seed and the
component that consumes it from drifting apart byte-for-byte and silently
missing the cache — the failure the SvelteKit example used to have built in,
with the same `SELECT` written out in both `+page.ts` and `+page.svelte`. It is
also what `db.run` (which unwraps a single-statement result), `db.watch` and
`db.invalidate` take.

## Where the generated file goes

`generate` with no `--out` writes to the **workspace root**, which is not where
either example imports it from. Each project pins the path in `package.json`, so
there is one command and one location:

| Example | `--out` | Imported as |
| --- | --- | --- |
| `basic/` | `src/surrealguard.generated.ts` | `./surrealguard.generated` |
| `sveltekit/` | `src/lib/surrealguard.generated.ts` | `$lib/surrealguard.generated` |

## Run the round-trip

From the repo root (the workspace links `@surrealguard/*`):

```sh
pnpm install

# 1. Build the CLI.
CARGO_TARGET_DIR=/tmp/sg cargo build --release -p surrealguard

# 2. Generate the typed registry from the schema + the project's queries.
cd examples/basic
/tmp/sg/release/surrealguard generate --out src/surrealguard.generated.ts

# 3. Type-check — the generated types make everything typed.
cd ../..
pnpm --filter @surrealguard-example/basic run typecheck
```

(For `examples/sveltekit` the flag is `--out src/lib/surrealguard.generated.ts`.)

Both examples' committed generated files are byte-identical to
what step 2 produces, so you can verify the round-trip by running it and
checking `git diff` is empty.

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
import { RecordId } from "./surrealguard.generated";
await db.run(peopleOf, { team: new RecordId("team", "red") });
```

## What the examples deliberately cannot show

A query text the registry does not contain is a hard error carrying its own
remedy (`SurqlError<"this query is not in the generated registry - run
\`surrealguard generate\`">`) rather than a silent degrade to `unknown[]`.

That guarantee cannot be demonstrated *in* a generated example, and the reason
is structural: `generate` extracts every `defineQuery` / `defineLive` literal in
the project, so any query written in an example is by construction in the
example's generated file. A registry miss is therefore only ever a **stale**
registry — and an example that regenerates cleanly is precisely one that cannot
hold a stale one. Reflowing a literal is the same case, not a different one: it
changes the key, so it errors until the next `generate`, which then picks the
reflowed text up as its own entry.

So the guarantee lives where the registry is hand-declared and no tool rewrites
it: [`packages/client/test-d/query.test-d.ts`](../packages/client/test-d/query.test-d.ts).
What the examples show instead is the deliberate opt-out, `defineQuery.unchecked`,
which degrades to `unknown[]` — never `any`.

## Type-checking

`examples/basic` runs `tsc`. `examples/sveltekit` runs `svelte-check`, so its
`.svelte` components are checked too — including that a bare `db.query("…")` in
`src/routes/+page.svelte` really resolves the row type inside the `{#each}`, and
that `people.data` in `src/routes/live/+page.svelte` really is
`Array<{ age: number; id: \`person:${string}\`; name: string; team: \`team:${string}\` }>`.

A `.svelte` claim has to be checked by `svelte-check`; `tsc` on a `.ts` file
proves nothing about it. In particular a `<script>` **without** `lang="ts"` is
not typechecked at all — `svelte-check` reports zero errors on
`people[0].nope.definitely.not.a.field` — which is why every snippet in these
docs carries `lang="ts"`.
