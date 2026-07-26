# @surrealguard/next

Next.js / React bindings for SurrealGuard: live SurrealQL queries as a hook,
typed from your schema. `useLiveQuery` returns `{ data, status, error }` backed
by `useSyncExternalStore`, and keeps `data` reconciled as `LIVE SELECT`
notifications arrive.

`react >= 18` and `surrealdb` are peer dependencies. Nothing is typed until you
run `surrealguard generate`, so the setup below starts there.

## Install

```sh
npm i @surrealguard/next @surrealguard/client @surrealguard/query surrealdb
npm i -D surrealguard
```

## 1. Describe your schema

`npx surrealguard init` writes a commented `surrealguard.toml`. Point its
`schema` glob at your `.surql` files, and ignore Next's build output:

```toml
# surrealguard.toml
[sources]
schema = ["schema/**/*.surql"]
queries = ["queries/**/*.surql"]
ignore = ["node_modules/**", ".next/**"]
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
npx surrealguard generate --out lib/surrealguard.generated.ts
```

`generate` scans your `.ts`/`.tsx` files for ``db.live(`…`)`` and `db.query("…")`
calls, analyzes each against the schema, and writes a module that re-exports
`SurrealGuardClient` plus a type registry keyed by each query's exact text.
Without `--out` it writes `surrealguard.generated.ts` at the workspace root; the
path above just keeps it next to `lib/db.ts` so the import is relative and
independent of how your `paths` aliases are configured.

Re-run it whenever a query or the schema changes. `generate` and `check` support
a watch mode (`--watch`) that stays running and regenerates on save; check
`surrealguard generate --help` for the flags your installed version has.

## 3. Create the client

```ts
// lib/db.ts
import { SurrealGuardClient } from "./surrealguard.generated";

export const db = new SurrealGuardClient();
```

Import the client **from the generated file**. That import is what loads the type
registry; importing it from `@surrealguard/client` instead leaves the registry
empty and every query degrades to `unknown`.

## 4. Provide it

`SurrealGuardProvider` is a React context provider, so it has to live in a client
component. Wrap the app once and every `useLiveQuery` below it resolves the
client without props threading.

```tsx
// app/providers.tsx
"use client";
import { SurrealGuardProvider } from "@surrealguard/next";
import { db } from "@/lib/db";

export function Providers({ children }: { children: React.ReactNode }) {
  return <SurrealGuardProvider client={db}>{children}</SurrealGuardProvider>;
}
```

Render `<Providers>` inside your root layout. If it is missing, `useLiveQuery`
throws `[@surrealguard/next] No client in context…` rather than failing later
with an undefined-property error.

## 5. Query

```tsx
// app/people/people-list.tsx
"use client";
import { useLiveQuery } from "@surrealguard/next";

export function PeopleList({ initialData }: { initialData?: { name: string; age: number }[] }) {
  // `db` is the context client. The row type comes from the generated registry.
  const { data, status } = useLiveQuery((db) => db.live(`SELECT name, age FROM person`), {
    initialData,
  });

  if (status === "loading") return <p>Loading…</p>;
  return (
    <ul>
      {data.map((person) => (
        <li key={person.name}>
          {person.name} — {person.age}
        </li>
      ))}
    </ul>
  );
}
```

Subscriptions are reference-counted per `(sql, params)`, so ten components
watching the same query share one `LIVE SELECT` and one reconciled array; the
last unmount tears it down.

Note the parentheses in ``db.live(`…`)``: the query is an *argument*, not a
tagged template. TypeScript widens a tagged template's text to `string`, which
would throw the row type away. A query that is not in the registry degrades to
`unknown` — never `any`.

## RSC: seed on the server, go live on the client

`@surrealguard/next/server` carries no `"use client"`, so it is safe to import
from a Server Component, a route handler, or `getServerSideProps`. A
`LIVE SELECT` cannot resolve rows in one shot, so `queryServer` runs its
underlying `SELECT` once and returns typed rows — pass them to `useLiveQuery`'s
`initialData` for a first paint with no loading state.

```tsx
// app/people/page.tsx  (Server Component)
import { queryServer } from "@surrealguard/next/server";
import { db } from "@/lib/db";
import { PeopleList } from "./people-list";

export default async function Page() {
  const people = await queryServer(db, db.live(`SELECT name, age FROM person`));
  return <PeopleList initialData={people} />;
}
```

To move a whole cache rather than one query's rows, call `dehydrate(db)` on the
server and `hydrate(db, state)` on the client.

## Escape hatches

- **No context** — `useLiveQuery(fn, { client })` uses the client you pass and
  never reads context. Useful in tests, in a subtree outside the provider, or
  when you hold two connections.
- **Bindings** — `useLiveQuery(fn, { params })`. Params take part in the cache
  key, so different params get their own subscription, and changing them
  re-observes.
- **A different generated path** — `generate --out <path>` takes any location.

## API

| Export | Entry | What it does |
| --- | --- | --- |
| `SurrealGuardProvider` | `@surrealguard/next` | Put the client in React context. |
| `useClient(override?)` | `@surrealguard/next` | Read it back. Throws a named error if absent. |
| `useLiveQuery(fn, options?)` | `@surrealguard/next` | Reactive `{ data, status, error }`. |
| `queryServer(db, descriptor, params?)` | `@surrealguard/next/server` | Run a live query's `SELECT` once (RSC seed). |
| `dehydrate(db)` / `hydrate(db, state)` | `@surrealguard/next/server` | Move the whole result cache across the boundary. |

Built on [`@surrealguard/query`](https://www.npmjs.com/package/@surrealguard/query),
which holds the cache, the reference counting, and the live reconciliation.

Part of [SurrealGuard](https://github.com/DrewRidley/surrealguard). Licensed
under MIT OR Apache-2.0.
