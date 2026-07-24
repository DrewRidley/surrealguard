<script lang="ts">
  // Reads the client from context (via liveQuery → getClient) and renders the
  // runes-reactive `users.data` DIRECTLY — no store `$` prefix.
  import { liveQuery } from "../src/index.js";

  let { initial = undefined }: { initial?: Array<Record<string, unknown>> } = $props();

  const users = liveQuery((db) => db.live(`SELECT * FROM user`), { initial });
</script>

<ul>
  {#each users.data as user (user.id)}
    <li>{String(user.name)}</li>
  {/each}
</ul>
