<script lang="ts">
  // Reads the client from context (via createLive -> useClient) and renders the
  // reactive `users.data` DIRECTLY — no store `$` prefix.
  //
  // Everything reaches `createLive` through a THUNK, which is the whole point
  // of `Source<Q>`: changing `team` (or the route's preloaded `data`)
  // re-resolves the query and re-subscribes. In 0.4 params were read once at
  // construction, so this could not work at all.
  import type { Json, Preloaded } from "@surrealdb/analyzer-client";
  import { createLive } from "../src/index.js";
  import { liveUsers, liveUsersOfTeam } from "./queries.js";

  type Row = Json<{ id: import("@surrealdb/analyzer-client").RecordId<"user">; name: string }>;

  let {
    team = undefined,
    preloaded = undefined,
  }: { team?: string; preloaded?: Preloaded<Row[]> } = $props();

  const users = createLive(
    () => preloaded ?? (team ? liveUsersOfTeam.with({ team }) : liveUsers),
  );
</script>

<p data-testid="status">{users.status}</p>
{#if users.error}
  <p data-testid="error">{users.error.message}</p>
{/if}
<ul>
  {#each users.data as user (user.id)}
    <li>{user.name}</li>
  {/each}
</ul>
