<!--
  The pattern `<LiveQuery>` exists for: an outer one-shot `<Query>` whose rows
  each mount their own live subscription. `visible` stands in for a viewport —
  narrowing it unmounts rows, which is what the teardown test drives.

  Both snippet parameters are written with no annotation. That they are typed at
  all is what `test-d/Components.svelte` proves; here it only has to run.
-->
<script lang="ts">
  import { LiveQuery, Query } from "../src/index.js";
  import { allUsers, liveUserNamed } from "./queries.js";

  let { visible }: { visible: string[] } = $props();
</script>

<Query q={allUsers}>
  {#snippet loading()}<p data-testid="outer-loading">…</p>{/snippet}
  {#snippet children(users)}
    <ul>
      {#each users.filter((user) => visible.includes(user.name)) as user (user.id)}
        <LiveQuery q={liveUserNamed.with({ name: user.name })}>
          {#snippet loading()}<li data-testid="pending-{user.name}">…</li>{/snippet}
          {#snippet children(rows)}
            <li data-testid="row-{user.name}">{rows.map((row) => row.name).join(",")}</li>
          {/snippet}
        </LiveQuery>
      {/each}
    </ul>
  {/snippet}
</Query>
