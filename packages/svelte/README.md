# @surrealguard/svelte

Svelte 5 / SvelteKit bindings for SurrealGuard.

**You may not need this package.** The typed client works in a component with
nothing around it — no provider, no wrapper, no helper:

```svelte
<script lang="ts">
  import { db } from "$lib/db";

  const rows = db.query("SELECT id, name, age, team FROM person");
</script>

{#await rows then [people]}
  <ul>
    {#each people as person (person.id)}
      <li>{person.name} — {person.age}</li>
    {/each}
  </ul>
{/await}
```

`person.name` is a `string`, `person.team` is a `RecordId<"team">` and
`person.nope` is a compile error, all inferred from your schema.

What this package adds is *reactive* state: a live query that re-renders as rows
change, a query whose parameters follow `$derived` state, an SSR payload a
component can pick up without naming the query twice. Reach for it when you want
one of those.

## Setup

Three steps, and skipping any of them yields `any` with no error on your own
code — see [When everything is `any`](#when-everything-is-any).

**1. Install.** All three, including `@surrealguard/client`: the generated file
augments that module *by name*, and if the name does not resolve the whole
registry is silently dropped.

```sh
npm install @surrealguard/svelte @surrealguard/client surrealdb
npm install -D surrealguard
```

**2. Generate into `src/lib`,** so `$lib/surrealguard.generated` resolves. Bare
`generate` writes to the workspace root, which is not where that import points:

```sh
npx surrealguard generate --out src/lib/surrealguard.generated.ts
```

Put it in `package.json` so the path is written once:

```json
{ "scripts": { "generate": "surrealguard generate --out src/lib/surrealguard.generated.ts" } }
```

Commit the generated module — it is what makes a fresh checkout type-check
without a build step. Re-run on every schema or query change, or leave
`--watch` running.

**3. Create the client once.**

```ts
// src/lib/db.ts
import { createClient } from "$lib/surrealguard.generated";

export const db = createClient({
  url: "ws://localhost:8000/rpc",
  namespace: "app",
  database: "app",
});
```

Import `createClient` **from the generated file** — that import is what loads the
registry augmentation. The connection opens lazily, so a module-level `db` is
safe and nothing has to `await db.connect(...)`.

That is setup finished. `db.query`, `db.run` and `db.watch` now work from a plain
`import { db } from "$lib/db"` in any component, `load`, server route or `.ts`
module.

## `setClient` is not part of setup

It is easy to read the old docs and conclude the client has to be *provided*
before anything works. It does not. `setClient` exists for one thing: the
reactive helpers below (`createQuery`, `createLive`, `createMutation`) resolve
their client from Svelte context, so components do not thread `db` through
props.

```svelte
<!-- src/routes/+layout.svelte — only if you use the reactive helpers -->
<script lang="ts">
  import { setClient } from "@surrealguard/svelte";
  import { db } from "$lib/db";

  setClient(db);
  let { children } = $props();
</script>

{@render children()}
```

Every helper also takes the client directly, which is the same thing without the
context hop — and it is what a `.svelte.ts` module must do anyway, since
`setContext` is only readable during component initialisation:

```ts
const people = createLive(livePeople, { client: db });
```

Use whichever you prefer. Neither is more typed than the other.

## Naming queries — for when two places must agree

`db.query("…")` takes the text inline. `defineQuery` / `defineLive` name it, and
what that buys is a *value* rather than any extra type safety — they read the
same registry through the same conditional generic. It matters when the same
query appears in two files, which is exactly the SSR case:

```ts
// src/lib/queries.ts
import { defineQuery, defineLive } from "$lib/surrealguard.generated";

export const allPeople  = defineQuery("SELECT id, name, age, team FROM person");
export const addPerson  = defineQuery("CREATE person SET name = $name, age = $age, team = $team");
export const livePeople = defineLive("SELECT id, name, age, team FROM person");
export const liveTeam   = defineLive("SELECT id, name FROM person WHERE team = $team");
```

A live query needs a name in any case: `createLive` has to hold a reference to
re-subscribe with.

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
  <ul>
    {#each people.data as person (person.id)}
      <li>{person.name}</li>
    {/each}
  </ul>
{/if}
```

`people.data` is ``Array<{ age: number; id: `person:${string}`; name: string; team: `team:${string}` }>``.
No `$` prefix — reading a getter tracks. `data` is always an array and starts
`[]`, so markup never needs `?? []`. N components sharing a query share one
`LIVE SELECT`, and the last one to unmount issues the `KILL`.

### Reactive parameters

**Wrap the query in a function.** A thunk re-runs when its dependencies change,
so the query re-subscribes:

```svelte
<script lang="ts">
  import { createLive } from "@surrealguard/svelte";
  import { liveTeam } from "$lib/queries";
  import { RecordId } from "$lib/surrealguard.generated";

  let { slug }: { slug: string } = $props();

  // when `slug` changes, the old subscription is KILLed and a new one opens
  const roster = createLive(() => liveTeam.with({ team: new RecordId("team", slug) }));
</script>
```

In a SvelteKit route `slug` is usually `page.params.team` from `$app/state` —
same rule, and the same reason it must be read *inside* the thunk. This is what
`@tanstack/svelte-query` v6 and `convex-svelte` both arrived at: *the argument
must be wrapped in a function to preserve reactivity*.

### Conditional queries

`"skip"` says "not yet", and keeps the row type:

```svelte
<script lang="ts">
  import { createLive } from "@surrealguard/svelte";
  import { liveTeam } from "$lib/queries";
  import { RecordId } from "$lib/surrealguard.generated";

  let { slug }: { slug: string | undefined } = $props();

  const roster = createLive(() =>
    slug ? liveTeam.with({ team: new RecordId("team", slug) }) : "skip",
  );
</script>
```

## `createQuery` — a one-shot query with loading and error state

```svelte
<script lang="ts">
  import { createQuery } from "@surrealguard/svelte";
  import { allPeople } from "$lib/queries";

  const roster = createQuery(allPeople);
</script>

{#if roster.loading}
  <p>Loading…</p>
{:else if roster.error}
  <p>{roster.error.message}</p>
{:else}
  <ul>
    {#each roster.data ?? [] as person (person.id)}
      <li>{person.name}</li>
    {/each}
  </ul>
{/if}
```

`data` is `T | undefined` here, because a one-shot query's result may be a
scalar (`RETURN count(…)`) and there is nothing honest to default it to. If you
only need the rows once and do not need loading state, `{#await db.query("…")}`
is less machinery.

## `createMutation` — a write, and what it invalidates

```svelte
<script lang="ts">
  import { createMutation } from "@surrealguard/svelte";
  import { addPerson, allPeople, livePeople } from "$lib/queries";
  import { RecordId } from "$lib/surrealguard.generated";

  const add = createMutation(addPerson, { invalidates: [allPeople, livePeople] });
</script>

<button
  onclick={() => add.mutate({ name: "ada", age: 36, team: new RecordId("team", "red") })}
  disabled={add.pending}>Add</button>
{#if add.error}<p class="error">{add.error.message}</p>{/if}
```

`mutate` is fire-and-forget (errors land on `.error`); `mutateAsync` returns the
result and throws.

## SSR — `preload`

This is where a named query earns its keep, and it is the one thing `db.query`
cannot do for you.

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
  import type { load } from "./+page";

  let { data }: { data: Awaited<ReturnType<typeof load>> } = $props();

  const people = createLive(() => data.people);
</script>

<ul>
  {#each people.data as person (person.id)}
    <li>{person.name}</li>
  {/each}
</ul>
```

(A real app writes `import type { PageData } from "./$types"`, which SvelteKit
generates as exactly that type.)

The payload carries its own key, text and params, so the component subscribes to
*exactly* the query the server ran. It renders from the seed on the first paint
and upgrades to live in place. Write the text out in both files instead and a
one-byte drift silently discards the seed — no error, no warning, no type
failure. That is the flaw this package was rebuilt around.

Wrap it in a thunk (`() => data.people`) so a client-side navigation that
replaces `data` re-runs it rather than pinning the first payload forever.

## When everything is `any`

Four ways to get `any` with no error on your own code. All four have bitten a
real reader of these docs:

1. **`@surrealguard/client` is not installed.** The generated file says
   `declare module "@surrealguard/client"`. If the specifier does not resolve,
   TypeScript reports `TS2664` **inside the generated file** — which you would
   never open — and drops the entire registry.
2. **No `lang="ts"` on the `<script>` tag.** Svelte does not typecheck an
   untyped script block at all: `svelte-check` reports *zero errors* on
   `people[0].nope.definitely.not.a.field`. Every snippet above says `lang="ts"`
   because people copy the whole block.
3. **The generated file is somewhere else.** Bare `generate` writes to the
   workspace root, not `src/lib`. If your import says `$lib/surrealguard.generated`
   and the file is at the root, you now have two of them and they will drift.
4. **You imported `createClient` / `defineQuery` from `@surrealguard/client`**
   rather than from the generated file. The augmentation loads with the import.

## Values are JSON here

The reactive layer is `Json<T>`-shaped: a `RecordId` arrives as
`` `person:${string}` ``, a `datetime` as an ISO string. That is what survives
devalue and what `JSON.stringify` produces, so an SSR payload needs no special
handling.

`db.query(…)` and `db.run(…)` outside the reactive layer give the SDK's real
values (`RecordId`, `Date`) instead. The two differ, deliberately: a React
Server Component boundary rejects class instances and has no transport hook, so
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
import { db } from "$lib/db";
import { livePeople } from "$lib/queries";

export const people = createLive(livePeople, { client: db });
```

Pass `{ client }` there: `setContext` is only readable during component
initialisation, so a module cannot read it.

## API

| Export | |
| --- | --- |
| `createLive(source, options?)` | live query; `data` is always an array |
| `createQuery(source, options?)` | one-shot; `data` is `T \| undefined` |
| `createMutation(query, options?)` | write + invalidation |
| `preload(db, query)` | SSR payload that remembers its query |
| `setClient(db)` / `useClient(override?)` | context, for the helpers above |
| `dehydrate(db)` / `hydrate(db, state)` | whole-cache transport |
| `transport` (`/transport`) | SvelteKit hook for SDK value classes |
| `Source<Q>` | `Q \| (() => Q \| "skip") \| "skip"` |

Every `create*` accepts `{ client }` as an alternative to context.
`create*` for reactive primitives, `use*` for context — TanStack Svelte v6's
split.

## Licence

MIT OR Apache-2.0
