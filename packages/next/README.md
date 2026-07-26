# @surrealguard/next

Next.js / React bindings for SurrealGuard. Typed queries as hooks, with the
query text written exactly once.

```tsx
"use client";
import { useLive } from "@surrealguard/next";
import { livePeople } from "@/lib/queries";

export function People() {
  const people = useLive(livePeople);
  return <ul>{people.data.map((p) => <li key={p.id}>{p.name}</li>)}</ul>;
}
```

`people.data` is `Array<{ id: `person:${string}`; name: string; age: number }>`,
inferred from your schema.

## Install

```sh
npm install @surrealguard/next @surrealguard/client surrealdb
```

## Setup

```ts
// lib/queries.ts — the one place query text lives
import { defineQuery, defineLive } from "@/surrealguard.generated";

export const allPeople  = defineQuery("SELECT id, name, age FROM person");
export const addPerson  = defineQuery("CREATE person SET name = $name, joined = $joined");
export const livePeople = defineLive("SELECT id, name, age FROM person");
export const liveTeam   = defineLive("SELECT id, name FROM person WHERE team = $team");
```

### The server client must be per-request

```ts
// lib/db.server.ts
import { cache } from "react";
import { createClient } from "@/surrealguard.generated";

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

### The browser client

```tsx
// app/providers.tsx
"use client";
import { SurrealGuardProvider } from "@surrealguard/next";
import { createClient } from "@/surrealguard.generated";

const db = createClient({ url: process.env.NEXT_PUBLIC_SURREAL_URL! });

export function Providers({ children }: { children: React.ReactNode }) {
  return <SurrealGuardProvider client={db}>{children}</SurrealGuardProvider>;
}
```

A module-level client is correct here: the browser is one user, one session.

## Reading data in a Server Component

The default App Router pattern — await it in an RSC, ship zero client JS —
needs nothing from this package:

```tsx
// app/people/page.tsx
import { getDb } from "@/lib/db.server";
import { allPeople } from "@/lib/queries";

export default async function Page() {
  const people = await getDb().runJson(allPeople);
  return <ul>{people.map((p) => <li key={p.id}>{p.name}</li>)}</ul>;
}
```

Use `runJson`, not `run`, for anything you pass to a client component. An RSC
boundary accepts only plain values and offers no transport hook, so a `RecordId`
instance crossing it throws *"Only plain objects can be passed to Client
Components"*. `runJson` gives you `` `person:${string}` `` and ISO strings.

## Seeding a live client component — `preload`

```tsx
// app/people/page.tsx  (Server Component)
import { preload } from "@surrealguard/next/server";
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
import { useLive, useMutation, type Preloaded, type Json } from "@surrealguard/next";
import { addPerson, allPeople, livePeople } from "@/lib/queries";

type Person = Json<{ id: string; name: string; age: number }>;

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
client component nowhere.

This is the flaw the package was rebuilt around. Before, the RSC and the client
component each spelled the query out; change one and the key stopped matching,
so the seed was silently discarded and the page refetched, with no error and no
type failure.

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
does not arise. (`@surrealguard/svelte` does need a thunk — the frameworks
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
add.mutate({ name: "ada", joined: new Date() });      // errors land on .error
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

## Values are JSON in the hooks

The reactive layer is `Json<T>`-shaped: a `RecordId` arrives as
`` `person:${string}` ``, a `datetime` as an ISO string. That is not a
preference — it is what an RSC boundary accepts at all.

`getDb().run(allPeople)` gives the SDK's real values (`RecordId`, `Date`) for
server-only use.

## API

| Export | Entry | |
| --- | --- | --- |
| `SurrealGuardProvider` / `useClient` | `.` | context |
| `useLive(source, options?)` | `.` | live query; `data` is always an array |
| `useQuery(source, options?)` | `.` | one-shot; `data` is `T \| undefined` |
| `useMutation(query, options?)` | `.` | write + invalidation |
| `preload(db, query)` | `./server` | seed a client component |
| `dehydrate` / `hydrate` | `./server` | whole-cache transport |

`@surrealguard/next/server` carries no `"use client"` directive, so it is safe
in an RSC.

## Licence

MIT OR Apache-2.0
