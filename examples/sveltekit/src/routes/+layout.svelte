<!--
  `setClient` is NOT the price of entry, and the docs used to read as if it
  were. `db.query`, `db.run` and `db.watch` work from a plain
  `import { db } from "$lib/db"` anywhere — a component, a `load`, a server
  route, a plain `.ts` module. Nothing has to be provided or wired up first.

  This call exists for exactly one thing: the REACTIVE helpers (`createQuery`,
  `createLive`, `createMutation`) resolve their client from Svelte context, so
  that components do not thread `db` through props. Every one of them also
  accepts `{ client: db }` directly — which is what a `.svelte.ts` module must
  do anyway, since `setContext` is only readable during component
  initialisation.

  So: using `createLive` in components? Call this once, here. Otherwise delete
  it.
-->
<script lang="ts">
  import { setClient } from "@surrealguard/svelte";
  import { db } from "$lib/db";

  setClient(db);

  let { children } = $props();
</script>

{@render children()}
