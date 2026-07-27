# @surrealguard/query

The framework-agnostic reactive core behind `@surrealguard/svelte` and
`@surrealguard/next`. Use it directly if you are writing your own binding, or
want a cache without a framework.

**Most people do not need this package.** For typed rows, `db.query("…")` in
`@surrealguard/client` is the whole API:

```ts
const [people] = await db.query("SELECT id, name, age FROM person");
//     ^? Array<{ id: RecordId<"person">; name: string; age: number }>
```

For a single live subscription and nothing else, `db.watch(livePeople, render)`
is simpler than anything here and needs no cache. What this package adds is
**deduplication, caching and invalidation** across many subscribers.

```ts
import { getQueryClient } from "@surrealguard/query";
import { db } from "./db";
import { livePeople } from "./queries";

const people = getQueryClient(db).observeLive(livePeople);
const stop = people.subscribe((state) => {
  if (state.status === "success") render(state.data);
});
```

## Getting `db` and `livePeople`

Both come from the module SurrealGuard generates off your schema. Install the
client (the generated file augments it *by name*, so it must resolve) and
generate with an `--out` that matches how you import it — bare `generate` writes
to the workspace root, which is usually not where your import points:

```sh
npm install @surrealguard/query @surrealguard/client surrealdb
npm install -D surrealguard
npx surrealguard generate --out src/surrealguard.generated.ts
```

```ts
// src/db.ts
import { createClient } from "./surrealguard.generated";
export const db = createClient({ url: "ws://localhost:8000/rpc" });
```

```ts
// src/queries.ts
import { defineLive } from "./surrealguard.generated";
export const livePeople = defineLive("SELECT id, name, age FROM person");
```

Import `createClient` and `defineLive` **from the generated file** — that is
what loads the registry augmentation. If results come back `any` rather than
typed, see [When everything is `any`](../client/README.md#when-everything-is-any)
in the client README.

## What it does

- **Deduplicates.** N subscribers to the same query share one cache entry, one
  `LIVE SELECT`, and one reconciled array. The last unsubscribe issues one
  `KILL`.
- **Reconciles.** A live query seeds from its underlying `SELECT` — a
  `LIVE SELECT` never replays existing records — then applies notifications by
  record `id`: CREATE appends, UPDATE replaces, DELETE removes.
- **Caches by the query's own key.** The key is `text` plus stably serialised
  params, computed once on the query reference. Nothing re-derives it, so an SSR
  seed and a client subscription cannot disagree about which entry they mean.
- **Bounds itself.** Entries are dropped `gcTime` after their last subscriber
  leaves, and the cache is capped at `maxEntries`.

## Everything here is `Json<T>`

Rows pass through the SDK's `jsonify` on the way in, so a `RecordId` is already
the `` `person:${string}` `` string it will be on the far side of an SSR
boundary — SvelteKit's devalue and a React Server Component's props both reject
class instances, and the RSC boundary has no transport hook to widen.

```ts
const people = await db.run(allPeople);
//    ^? Array<{ id: RecordId<"person">; … }>       — SDK values
const state = getQueryClient(db).observe(allPeople).get();
//    state.data ^? Array<{ id: `person:${string}`; … }>   — JSON projection
```

`db.run` gives SDK values; this layer gives their JSON projection. That is a
real inconsistency, and it is the deliberate one: uniformity here would either
break SSR or break Next entirely.

## State is a discriminated union

```ts
type QueryState<T> =
  | { status: "pending"; data: T | undefined; error: undefined }
  | { status: "success"; data: T;             error: undefined }
  | { status: "error";   data: T | undefined; error: SurrealGuardError };
```

`status` narrows `data`, so reading rows before checking the status is a type
error rather than a silently empty array.

## API

```ts
const qc = getQueryClient(db);            // one core per client, cached

qc.observe(allPeople)                     // one-shot; result may be a scalar
qc.observeLive(livePeople)                // live; data is always an array
qc.fetch(allPeople)                       // run once, outside the reactive layer
qc.prime(livePeople)                      // run a live query's SELECT once (SSR seed)

qc.getData(allPeople.key)                 // types itself from the branded key
qc.setData(allPeople, (prev) => prev ?? []) // optimistic write
qc.refetch(allPeople)
qc.invalidate(allPeople)                  // every binding of that query
qc.invalidate(peopleOf.with({ team }))    // exactly that binding
qc.mutate(addPerson, { name }, { invalidates: [allPeople] })

qc.dehydrate()                            // snapshot for the SSR payload
qc.hydrate(snapshot)
```

`db.invalidate(...)` on the client reaches this cache too — the client publishes
invalidations and the core subscribes, so the client never has to import the
reactive layer.

### Invalidation granularity

An unbound query's key is its text; a bound one's is `text::params`. Matching is
by key prefix, so both granularities fall out of the same rule:

```ts
qc.invalidate(peopleOf);                   // all teams
qc.invalidate(peopleOf.with({ team }));    // just that team
```

### Bounding the cache

```ts
new QueryClient(db, { gcTime: 60_000, maxEntries: 200 });
```

`gcTime: Infinity` keeps entries forever.

## Observing a scalar

A one-shot query's result is whatever the query returns:

```ts
const count = getQueryClient(db).observe(peopleCount);
count.get().data;   // ^? number | undefined
```

## Licence

MIT OR Apache-2.0
