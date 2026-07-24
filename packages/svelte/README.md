# @surrealguard/svelte

Svelte 5 (runes) bindings for SurrealGuard. Provide the typed client once via
context, then call `liveQuery` — it returns a runes-reactive object you read
DIRECTLY (`users.data`), no store `$` prefix. A `LIVE SELECT …` keeps `data`
reconciled as notifications arrive; the subscription is reference-counted and
torn down when the component is destroyed.

```svelte
<!-- +layout.svelte — provide the client once -->
<script>
  import { setClient } from "@surrealguard/svelte";
  import { SurrealGuardClient } from "$lib/surrealguard.generated";
  setClient(new SurrealGuardClient(/* connect... */));
</script>
<slot />
```

```svelte
<!-- +page.svelte — read reactive data directly -->
<script>
  import { liveQuery } from "@surrealguard/svelte";
  // `db` comes from context; row type is inferred from the query.
  const users = liveQuery((db) => db.live(`SELECT * FROM user`));
</script>

{#each users.data as user (user.id)}
  <li>{user.name}</li>
{/each}
```

`db.live(...)` takes the query as an argument (note the parentheses) so its
literal type — and thus the row type — is preserved. `users.data` is fully typed
for a registered query and `unknown[]` for a non-registered one (never `any[]`).

## SSR: gap-free first render, then live

Seed from a `load` with `loadLive`, which runs the underlying `SELECT` once, then
pass the rows to `liveQuery`'s `initial`. The client subscribes and upgrades to
live in place.

```ts
// +page.ts (or +page.server.ts)
import { loadLive } from "@surrealguard/svelte";
import { db } from "$lib/db";
export async function load() {
  const users = await loadLive(db, db.live(`SELECT * FROM user`));
  return { users };
}
```

```svelte
<!-- +page.svelte -->
<script>
  import { liveQuery } from "@surrealguard/svelte";
  let { data } = $props();
  const users = liveQuery((db) => db.live(`SELECT * FROM user`), { initial: data.users });
</script>
{#each users.data as user (user.id)}<li>{user.name}</li>{/each}
```

For whole-cache transport use `dehydrate(db)` on the server and `hydrate(db, state)`
on the client. Pass an explicit client with `liveQuery(fn, { client })` to bypass
context (e.g. tests, multiple connections).

Built on [`@surrealguard/query`](https://www.npmjs.com/package/@surrealguard/query).
`svelte ^5` is a peer dependency.

Part of [SurrealGuard](https://github.com/DrewRidley/surrealguard). Licensed
under MIT OR Apache-2.0.
