# @surrealguard/svelte

Svelte 5 / SvelteKit bindings for SurrealGuard. Typed queries as reactive
primitives, with the query text written exactly once.

```svelte
<script lang="ts">
  import { createLive } from "@surrealguard/svelte";
  import { livePeople } from "$lib/queries";

  const people = createLive(livePeople);
</script>

{#each people.data as person (person.id)}
  <li>{person.name} — {person.age}</li>
{/each}
```

`people.data` is `Array<{ id: `person:${string}`; name: string; age: number }>`,
inferred from your schema. No `$` prefix — reading a getter tracks.

## Install

```sh
npm install @surrealguard/svelte @surrealguard/client surrealdb
```

## Setup

```ts
// src/lib/db.ts
import { createClient } from "$lib/surrealguard.generated";

export const db = createClient({ url: "ws://localhost:8000/rpc" });
```

```ts
// src/lib/queries.ts — the one place query text lives
import { defineQuery, defineLive } from "$lib/surrealguard.generated";

export const allPeople  = defineQuery("SELECT id, name, age FROM person");
export const addPerson  = defineQuery("CREATE person SET name = $name, joined = $joined");
export const livePeople = defineLive("SELECT id, name, age FROM person");
export const liveTeam   = defineLive("SELECT id, name FROM person WHERE team = $team");
```

```svelte
<!-- src/routes/+layout.svelte -->
<script lang="ts">
  import { setClient } from "@surrealguard/svelte";
  import { db } from "$lib/db";

  setClient(db);
  let { children } = $props();
</script>

{@render children()}
```

## `createLive` — a live query

```svelte
<script lang="ts">
  import { createLive } from "@surrealguard/svelte";
  import { livePeople } from "$lib/queries";

  const people = createLive(livePeople);
</script>

{#if people.error}
  <p class="error">{people.error.message}</p>
{:else}
  <ul>{#each people.data as p (p.id)}<li>{p.name}</li>{/each}</ul>
{/if}
```

`data` is always an array and starts `[]`, so markup never needs `?? []`.
N components sharing a query share one `LIVE SELECT`, and the last one to
unmount issues the `KILL`.

## Reactive parameters

**Wrap the query in a function.** A thunk re-runs when its dependencies change,
so the query re-subscribes:

```svelte
<script lang="ts">
  import { page } from "$app/state";
  import { createLive } from "@surrealguard/svelte";
  import { liveTeam } from "$lib/queries";

  // navigating to another team KILLs the old subscription and opens a new one
  const team = createLive(() => liveTeam.with({ team: page.params.team }));
</script>
```

This is the same rule `@tanstack/svelte-query` v6 and `convex-svelte` both
arrived at: *the argument must be wrapped in a function to preserve
reactivity*.

### Conditional queries

`"skip"` says "not yet", and keeps the row type:

```ts
const team = createLive(() => (session ? liveTeam.with({ team }) : "skip"));
```

## `createQuery` — a one-shot query

```svelte
<script lang="ts">
  import { createQuery } from "@surrealguard/svelte";
  import { allPeople } from "$lib/queries";

  const roster = createQuery(allPeople);
</script>

{#if roster.loading}
  <Spinner />
{:else if roster.error}
  <p>{roster.error.message}</p>
{:else}
  <ul>{#each roster.data ?? [] as p (p.id)}<li>{p.name}</li>{/each}</ul>
{/if}
```

`data` is `T | undefined` here, because a one-shot query's result may be a
scalar (`RETURN count(…)`) and there is nothing honest to default it to.

## `createMutation` — a write, and what it invalidates

```svelte
<script lang="ts">
  import { createMutation } from "@surrealguard/svelte";
  import { addPerson, allPeople, livePeople } from "$lib/queries";

  const add = createMutation(addPerson, { invalidates: [allPeople, livePeople] });
</script>

<button onclick={() => add.mutate({ name: "ada", joined: new Date() })}
        disabled={add.pending}>Add</button>
{#if add.error}<p class="error">{add.error.message}</p>{/if}
```

`mutate` is fire-and-forget (errors land on `.error`); `mutateAsync` returns the
result and throws.

## SSR — `preload`

```ts
// src/routes/+page.ts
import { preload } from "@surrealguard/svelte";
import { db } from "$lib/db";
import { livePeople } from "$lib/queries";

export async function load() {
  return { people: await preload(db, livePeople) };
}
```

```svelte
<!-- src/routes/+page.svelte : the query text appears nowhere -->
<script lang="ts">
  import { createLive } from "@surrealguard/svelte";
  let { data } = $props();

  const people = createLive(() => data.people);
</script>

<ul>{#each people.data as p (p.id)}<li>{p.name}</li>{/each}</ul>
```

The payload carries its own key, text and params, so the component subscribes to
*exactly* the query the server ran. It renders from the seed on the first paint
and upgrades to live in place.

This is the flaw the package was rebuilt around. Before, `+page.ts` and
`+page.svelte` each spelled the query out; change one and the key stopped
matching, so the seed was silently discarded and the page refetched — no error,
no warning, no type failure.

Wrap it in a thunk (`() => data.people`) if the route's `data` can change under
you on a client-side navigation.

## Values are JSON here

The reactive layer is `Json<T>`-shaped: a `RecordId` arrives as
`` `person:${string}` ``, a `datetime` as an ISO string. That is what survives
devalue and what `JSON.stringify` produces, so an SSR payload needs no special
handling.

`db.run(allPeople)` outside the reactive layer gives the SDK's real values
(`RecordId`, `Date`) instead. The two differ, deliberately: a React Server
Component boundary rejects class instances and has no transport hook, so
uniformity in the other direction is not available.

### If you want SDK classes through `load`

devalue rejects class instances, so `load` cannot return a `RecordId` on its
own. Register the transport hook:

```ts
// src/hooks.ts
export { transport } from "@surrealguard/svelte/transport";
```

It covers `RecordId`, `DateTime`, `Duration`, `Uuid` and `Decimal`. Spread it to
add your own:

```ts
import { transport as surrealguard } from "@surrealguard/svelte/transport";
export const transport = { ...surrealguard, MyType: { encode, decode } };
```

## Sharing a query from a `.svelte.ts` module

The primitives use `createSubscriber`, not `$effect`, so they work outside a
component and tear down automatically:

```ts
// src/lib/people.svelte.ts
import { createLive } from "@surrealguard/svelte";
import { livePeople } from "$lib/queries";
import { db } from "$lib/db";

export const people = createLive(livePeople, { client: db });
```

(Pass `{ client }` explicitly there — `setContext` is only readable during
component initialisation.)

## API

| Export | |
| --- | --- |
| `setClient(db)` / `useClient(override?)` | context |
| `createLive(source, options?)` | live query; `data` is always an array |
| `createQuery(source, options?)` | one-shot; `data` is `T \| undefined` |
| `createMutation(query, options?)` | write + invalidation |
| `preload(db, query)` | SSR payload that remembers its query |
| `dehydrate(db)` / `hydrate(db, state)` | whole-cache transport |
| `transport` (`/transport`) | SvelteKit hook for SDK value classes |
| `Source<Q>` | `Q \| (() => Q \| "skip") \| "skip"` |

`create*` for reactive primitives, `use*` for context — TanStack Svelte v6's
split.

## Licence

MIT OR Apache-2.0
