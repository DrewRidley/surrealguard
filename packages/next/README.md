# @surrealdb/analyzer-next

Next.js / React bindings for SurrealQL Analyzer.

**You may not need this package.** The default App Router pattern — await the
query in a Server Component, ship zero client JS — is the typed client on its
own:

```tsx
// app/people/page.tsx
import { getDb } from "@/lib/db.server";

export default async function Page() {
  const [people] = await getDb().query("SELECT id, name, age FROM person");
  return <ul>{people.map((p) => <li key={String(p.id)}>{p.name}</li>)}</ul>;
}
```

`p.name` is a `string` and `p.nope` is a compile error, inferred from your
schema. No provider, no hook, no wrapper around the string.

What this package adds is *client-side reactive* state: a live query that
re-renders as rows change, and a way to seed one from the server without naming
the query twice. Reach for it when you want that.

## Setup

Three steps, and skipping any of them yields `any` with no error on your own
code — see [When everything is `any`](#when-everything-is-any).

**1. Install.** All three, including `@surrealdb/analyzer-client`: the generated file
augments that module *by name*, and if the name does not resolve the whole
registry is silently dropped.

```sh
npm install @surrealdb/analyzer-next @surrealdb/analyzer-client surrealdb
npm install -D surrealql-analyzer
```

**2. Generate where your `@/` alias points.** Bare `generate` writes to the
workspace root; check `paths` in `tsconfig.json` and match it. A
`create-next-app` project with a `src` directory maps `@/*` to `./src/*`:

```sh
npx surrealql-analyzer generate --out src/surrealql-analyzer.generated.ts   # with src/
npx surrealql-analyzer generate --out surrealql-analyzer.generated.ts       # without src/
```

Put it in `package.json` so the path is written once:

```json
{ "scripts": { "generate": "surrealql-analyzer generate --out src/surrealql-analyzer.generated.ts" } }
```

Commit the generated module — it is what makes a fresh checkout type-check
without a build step.

**3. Create the clients.** Next needs two, and this is the one piece of genuine
ceremony in the setup.

```ts
// lib/db.server.ts
import { cache } from "react";
import { createClient } from "@/surrealql-analyzer.generated";

export const getDb = cache(() =>
  createClient({
    url: process.env.SURREAL_URL!,
    namespace: "app",
    database: "app",
  }),
);
```

**Do not export a module-level client for server use.** Next imports that module
into the server runtime, so one connection — one auth session, one cache — would
be shared by every concurrent request and every user, and any `signin()` would
mutate global state for everyone. React's `cache()` scopes it to a request.

```tsx
// app/providers.tsx — only if you use the hooks below
"use client";
import { SurrealQLAnalyzerProvider } from "@surrealdb/analyzer-next";
import { createClient } from "@/surrealql-analyzer.generated";

const db = createClient({ url: process.env.NEXT_PUBLIC_SURREAL_URL! });

export function Providers({ children }: { children: React.ReactNode }) {
  return <SurrealQLAnalyzerProvider client={db}>{children}</SurrealQLAnalyzerProvider>;
}
```

A module-level client is correct here: the browser is one user, one session.
`SurrealQLAnalyzerProvider` is how `useLive` / `useQuery` / `useMutation` find a
client; each also takes `{ client }` directly if you would rather not use
context.

Import `createClient` **from the generated file** in both. That import is what
loads the registry augmentation; importing from `@surrealdb/analyzer-client` compiles
fine and gives you `unknown[]` forever.

## Reading data on the server

`query` resolves the per-statement tuple, exactly as SurrealDB returns it:

```tsx
const [people] = await getDb().query("SELECT id, name, age FROM person");
const [red] = await getDb().query("SELECT id, name FROM person WHERE team = $team", {
  team: new RecordId("team", "red"),
});
```

Anything you pass **to a client component** must go through `runJson` (or a
named query and `runJson`), not `run`/`query`. An RSC boundary accepts only
plain values and offers no transport hook, so a `RecordId` instance crossing it
throws *"Only plain objects can be passed to Client Components"*. `runJson`
gives you `` `person:${string}` `` and ISO strings:

```tsx
const people = await getDb().runJson(allPeople);
//    ^? Array<{ id: `person:${string}`; name: string; joined: string }>
```

## Naming queries — for when two places must agree

`defineQuery` / `defineLive` read the same registry through the same conditional
generic as `db.query`, so they add no type safety. They add a *value*: written
once, imported by both the Server Component that seeds a query and the client
component that subscribes to it, so the two cannot drift apart.

```ts
// lib/queries.ts
import { defineQuery, defineLive } from "@/surrealql-analyzer.generated";

export const allPeople  = defineQuery("SELECT id, name, joined FROM person");
export const addPerson  = defineQuery("CREATE person SET name = $name, joined = $joined");
export const livePeople = defineLive("SELECT id, name, joined FROM person");
export const liveTeam   = defineLive("SELECT id, name FROM person WHERE team = $team");
```

## Seeding a live client component — `preload`

This is the case that genuinely needs the package.

```tsx
// app/people/page.tsx  (Server Component)
import { preload } from "@surrealdb/analyzer-next/server";
import { getDb } from "@/lib/db.server";
import { livePeople } from "@/lib/queries";
import { PeopleList } from "./people-list";

export default async function Page() {
  const preloaded = await preload(getDb(), livePeople);
  return <PeopleList preloaded={preloaded} />;
}
```

```tsx
// app/people/people-list.tsx
"use client";
import { useLive, useMutation, type Preloaded, type Json } from "@surrealdb/analyzer-next";
import { addPerson, allPeople, livePeople } from "@/lib/queries";
import type { RecordId } from "@/surrealql-analyzer.generated";

type Person = Json<{ id: RecordId<"person">; name: string; joined: Date }>;

export function PeopleList({ preloaded }: { preloaded: Preloaded<Person[]> }) {
  const people = useLive(preloaded);        // hydrates, then upgrades to live
  const add = useMutation(addPerson, { invalidates: [allPeople, livePeople] });

  if (people.error) return <p>{people.error.message}</p>;
  return (
    <>
      <ul>{people.data.map((p) => <li key={p.id}>{p.name}</li>)}</ul>
      <button onClick={() => add.mutate({ name: "ada", joined: new Date() })}
              disabled={add.pending}>Add</button>
    </>
  );
}
```

The payload carries its own key, text and params, so the client component
subscribes to *exactly* the query the server ran — the text appears in the
client component nowhere. Write it out in both files instead and a one-byte
drift silently discards the seed: no error, no warning, no type failure. That is
the flaw this package was rebuilt around.

## Hooks

### `useLive` — a live query

```tsx
const people = useLive(livePeople);
const forTeam = useLive(liveTeam.with({ team }));
```

`data` is always an array and starts `[]`, so `.map(...)` needs no `?? []`.
N components sharing a query share one `LIVE SELECT`; the last unmount `KILL`s
it. Backed by `useSyncExternalStore`.

**No thunk.** A query reference carries a stable `key`, so the hook's memo
dependency is `[client, source.key]` and React's "did my deps change" problem
does not arise. (`@surrealdb/analyzer-svelte` does need a thunk — the frameworks
differ, so the APIs do.)

### `useQuery` — a one-shot query

```tsx
const roster = useQuery(allPeople);
if (roster.loading) return <Skeleton />;
if (roster.error) return <p>{roster.error.message}</p>;
return <ul>{roster.data?.map((p) => <li key={p.id}>{p.name}</li>)}</ul>;
```

`data` is `T | undefined`, because a one-shot query's result may be a scalar
(`RETURN count(…)`).

### Conditional queries

`"skip"` says "not yet", and keeps the row type:

```tsx
const forTeam = useLive(session ? liveTeam.with({ team }) : "skip");
```

### `useMutation` — a write and what it invalidates

```tsx
const add = useMutation(addPerson, { invalidates: [allPeople, livePeople] });
add.mutate({ name: "ada", joined: new Date() });             // errors land on .error
await add.mutateAsync({ name: "ada", joined: new Date() });  // throws
```

## Streaming a slow query

Pass an un-awaited promise from the server and `use()` it on the client — the
App Router idiom. (`use` is React 19; the rest of this package works on 18.)

```tsx
// page.tsx (server)
const rows = getDb().runJson(slowReport);   // deliberately not awaited
return <Suspense fallback={<Skeleton />}><Report rows={rows} /></Suspense>;
```

```tsx
// report.tsx
"use client";
import { use } from "react";

export function Report({ rows }: { rows: Promise<Row[]> }) {
  const data = use(rows);
  return <Table rows={data} />;
}
```

## When everything is `any`

Four ways to get `any` with no error on your own code:

1. **`@surrealdb/analyzer-client` is not installed.** The generated file says
   `declare module "@surrealdb/analyzer-client"`. If the specifier does not resolve,
   TypeScript reports `TS2664` **inside the generated file** — which you would
   never open — and drops the entire registry.
2. **The generated file is somewhere else.** Bare `generate` writes to the
   workspace root, which may not be where `@/` points. If your import and the
   file disagree, you end up with two generated files and they will drift.
3. **You imported `createClient` / `defineQuery` from `@surrealdb/analyzer-client`**
   rather than from the generated file. The augmentation loads with the import.
4. **The file is `.jsx`, or `checkJs` is off.** TypeScript does not check it.

## Values are JSON in the hooks

The reactive layer is `Json<T>`-shaped: a `RecordId` arrives as
`` `person:${string}` ``, a `datetime` as an ISO string. That is not a
preference — it is what an RSC boundary accepts at all.

`getDb().query(…)` and `getDb().run(…)` give the SDK's real values (`RecordId`,
`Date`) for server-only use.

## API

| Export | Entry | |
| --- | --- | --- |
| `SurrealQLAnalyzerProvider` / `useClient` | `.` | context, for the hooks below |
| `useLive(source, options?)` | `.` | live query; `data` is always an array |
| `useQuery(source, options?)` | `.` | one-shot; `data` is `T \| undefined` |
| `useMutation(query, options?)` | `.` | write + invalidation |
| `preload(db, query)` | `./server` | seed a client component |
| `dehydrate` / `hydrate` | `./server` | whole-cache transport |

`@surrealdb/analyzer-next/server` carries no `"use client"` directive, so it is safe
in an RSC.

## Licence

MIT OR Apache-2.0
