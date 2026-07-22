# @surrealguard/svelte

Svelte / SvelteKit bindings for SurrealGuard. A `load` function fetches typed
data; the page seeds `liveQuery` with it, which subscribes on mount, reconciles
live notifications, and unsubscribes automatically when the store loses its last
subscriber.

```svelte
<script>
  import { liveQuery } from "@surrealguard/svelte";
  export let data;
  const users = liveQuery(qc, "LIVE SELECT * FROM user", { initial: data.users });
</script>

{#each $users.data as user (user.id)}
  <li>{user.name}</li>
{/each}
```

Built on [`@surrealguard/query`](https://www.npmjs.com/package/@surrealguard/query).
`svelte >= 4` is a peer dependency.

Part of [SurrealGuard](https://github.com/DrewRidley/surrealguard). Licensed
under MIT OR Apache-2.0.
