<!--
  Two `<LiveQuery>` components on the SAME query, mounted independently. The
  cache is refcounted, so this must open one `LIVE SELECT`, not two — and must
  `KILL` it when the *second* one leaves, not the first.
-->
<script lang="ts">
  import { LiveQuery } from "../src/index.js";
  import { liveUsers } from "./queries.js";

  let { first, second }: { first: boolean; second: boolean } = $props();
</script>

{#if first}
  <LiveQuery q={liveUsers}>
    {#snippet children(rows)}<p data-testid="first">{rows.length}</p>{/snippet}
  </LiveQuery>
{/if}
{#if second}
  <LiveQuery q={liveUsers}>
    {#snippet children(rows)}<p data-testid="second">{rows.length}</p>{/snippet}
  </LiveQuery>
{/if}
