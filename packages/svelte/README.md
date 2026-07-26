# @surrealguard/svelte

Svelte 5 bindings for SurrealGuard: live SurrealQL queries as reactive state,
typed from your schema. `liveQuery` returns a `$state`-backed object you read
directly in markup — `people.data`, no store `$` prefix — and it keeps that array
reconciled as `LIVE SELECT` notifications arrive.

Requires Svelte 5 (runes). Nothing is typed until you run
`surrealguard generate`, so the setup below starts there.

## Install

```sh
npm i @surrealguard/svelte @surrealguard/client @surrealguard/query surrealdb
npm i -D surrealguard
```

`svelte ^5` and `surrealdb` are peer dependencies. `surrealguard` is the CLI that
generates the types; it downloads a prebuilt binary on first run.

## 1. Describe your schema

`npx surrealguard init` writes a commented `surrealguard.toml`. Point its
`schema` glob at your `.surql` files, and make sure SvelteKit's build output is
ignored:

```toml
# surrealguard.toml
[sources]
schema = ["schema/**/*.surql"]
queries = ["queries/**/*.surql"]
ignore = ["node_modules/**", ".svelte-kit/**", "build/**"]
```

```surql
-- schema/schema.surql
DEFINE TABLE team SCHEMAFULL;
DEFINE FIELD name ON team TYPE string;

DEFINE TABLE person SCHEMAFULL;
DEFINE FIELD name ON person TYPE string;
DEFINE FIELD age ON person TYPE int;
DEFINE FIELD team ON person TYPE record<team>;
```

## 2. Generate the typed client

```sh
npx surrealguard generate --out src/lib/surrealguard.generated.ts
```

`generate` scans your `.svelte` and `.ts` files for ``db.live(`…`)`` and
`db.query("…")` calls, analyzes each against the schema, and writes a module that
re-exports `SurrealGuardClient` plus a type registry keyed by each query's exact
text. Without `--out` it writes `surrealguard.generated.ts` at the workspace
root, which `$lib` cannot reach — `--out src/lib/…` puts it where `$lib`
resolves, so keep the flag.

Re-run it whenever a query or the schema changes. `generate` and `check` support
a watch mode (`--watch`) that stays running and regenerates on save; check
`surrealguard generate --help` for the flags your installed version has.

## 3. Create the client

```ts
// src/lib/db.ts
import { SurrealGuardClient } from "$lib/surrealguard.generated";

export const db = new SurrealGuardClient();
```

Import the client **from the generated file**. That import is what loads the type
registry; importing it from `@surrealguard/client` instead leaves the registry
empty and every query degrades to `unknown`.

## 4. Provide it once, at the root

`setClient` is a symbol-keyed `setContext`, so it has to run during component
initialisation in an ancestor of everything that queries — in practice, the root
`+layout.svelte`. Every `liveQuery` below it then resolves the client from
context, and no component has to thread `db` through props.

```svelte
<!-- src/routes/+layout.svelte -->
<script lang="ts">
  import { setClient } from "@surrealguard/svelte";
  import { db } from "$lib/db";

  setClient(db);

  // Connect in the browser only: a live subscription needs a long-lived socket,
  // which the server render does not have.
  $effect(() => {
    void db
      .connect("ws://localhost:8000/rpc")
      .then(() => db.use({ namespace: "app", database: "app" }));
    return () => void db.close();
  });

  let { children } = $props();
</script>

{@render children()}
```

If `setClient` never ran, `liveQuery` throws
`[@surrealguard/svelte] No client in context…` instead of failing later with an
undefined-property error.

## 5. Query

```svelte
<!-- src/routes/+page.svelte -->
<script lang="ts">
  import { liveQuery } from "@surrealguard/svelte";

  // `db` is the context client. The row type comes from the generated registry,
  // so `person.name` is `string` and `person.nope` is a compile error.
  const people = liveQuery((db) => db.live(`SELECT name, age FROM person`));
</script>

{#if people.loading}
  <p>Loading…</p>
{:else}
  <ul>
    {#each people.data as person (person.name)}
      <li>{person.name} — {person.age}</li>
    {/each}
  </ul>
{/if}
```

`liveQuery` returns `{ data, status, loading, error }` — read the fields
directly; they are runes-backed getters, not a store. The subscription starts in
an `$effect` and is torn down when the component is destroyed. Subscriptions are
reference-counted per `(sql, params)`, so ten components watching the same query
share one `LIVE SELECT` and one reconciled array.

Note the parentheses in ``db.live(`…`)``: the query is an *argument*, not a
tagged template. TypeScript widens a tagged template's text to `string`, which
would throw the row type away. A query that is not in the registry degrades to
`unknown` — never `any`.

## SSR: seed on the server, go live on the client

A `LIVE SELECT` cannot resolve rows in one shot, so `loadLive` runs its
underlying `SELECT` once and returns typed rows. Pass them to `liveQuery`'s
`initial` for a first paint with no loading gap; the client then upgrades the
same data to live in place.

```ts
// src/routes/+page.ts
import { loadLive } from "@surrealguard/svelte";
import { db } from "$lib/db";

export async function load() {
  const people = await loadLive(db, db.live(`SELECT name, age FROM person`));
  return { people };
}
```

```svelte
<!-- src/routes/+page.svelte -->
<script lang="ts">
  import { liveQuery } from "@surrealguard/svelte";

  let { data } = $props();

  const people = liveQuery((db) => db.live(`SELECT name, age FROM person`), {
    initial: data.people,
  });
</script>
```

To move a whole cache rather than one query's rows, call `dehydrate(db)` on the
server and `hydrate(db, state)` on the client.

## Escape hatches

- **No context** — `liveQuery(fn, { client })` uses the client you pass and never
  touches context. Useful in tests, in a component rendered outside the provider,
  or when you hold two connections.
- **Bindings** — `liveQuery(fn, { params })`. Params take part in the cache key,
  so different params get their own subscription.
- **A different generated path** — `generate --out <path>` takes any location;
  `$lib/surrealguard.generated` above is just what `src/lib/` resolves to.

## API

| Export | What it does |
| --- | --- |
| `setClient(db)` | Put the client in context. Call during init in the root layout. |
| `getClient()` | Read it back. Throws a named error if absent. |
| `liveQuery(fn, options?)` | Runes-reactive `{ data, status, loading, error }`. |
| `loadLive(db, descriptor, params?)` | Run a live query's underlying `SELECT` once (SSR seed). |
| `dehydrate(db)` / `hydrate(db, state)` | Move the whole result cache across the SSR boundary. |

Built on [`@surrealguard/query`](https://www.npmjs.com/package/@surrealguard/query),
which holds the cache, the reference counting, and the live reconciliation.

Part of [SurrealGuard](https://github.com/DrewRidley/surrealguard). Licensed
under MIT OR Apache-2.0.
