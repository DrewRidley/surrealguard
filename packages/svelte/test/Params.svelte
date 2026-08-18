<!-- A `<Query>` whose parameter is a prop, so a rerender changes it. -->
<script lang="ts">
  import { Query } from "../src/index.js";
  let { team }: { team: string } = $props();
</script>

<Query q="SELECT * FROM user WHERE team = $team" params={{ team }}>
  {#snippet loading()}<p data-testid="loading">…</p>{/snippet}
  {#snippet children(rows)}
    <p data-testid="rows">{rows.length}</p>
  {/snippet}
</Query>
