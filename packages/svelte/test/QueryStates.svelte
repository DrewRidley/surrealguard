<!--
  `<Query>`'s four states in one harness: loading, success, error-with-snippet,
  and error-with-NO-snippet — which is why the whole thing sits inside a
  `<svelte:boundary>`. An unclaimed error is thrown rather than swallowed, and
  the boundary is where a real app catches it, so the fallback policy is
  observable here instead of merely documented.
-->
<script lang="ts">
  import type { Preloaded, SurrealQLAnalyzerError } from "@surrealdb/analyzer-client";
  import { Query } from "../src/index.js";
  import { allUsers } from "./queries.js";

  type Row = { id: `user:${string}`; name: string };

  let {
    preloaded = undefined,
    handleError = true,
  }: { preloaded?: Preloaded<Row[]>; handleError?: boolean } = $props();
</script>

{#snippet failure(cause: SurrealQLAnalyzerError, retry: () => Promise<void>)}
  <p data-testid="error">{cause.message}</p>
  <button data-testid="retry" onclick={retry}>retry</button>
{/snippet}

<svelte:boundary>
  <Query q={preloaded ?? allUsers} error={handleError ? failure : undefined}>
    {#snippet loading()}<p data-testid="loading">loading</p>{/snippet}
    {#snippet children(users)}
      <ul>
        {#each users as user (user.id)}
          <li>{user.name}</li>
        {/each}
      </ul>
    {/snippet}
  </Query>

  {#snippet failed(cause)}
    <p data-testid="boundary">{(cause as Error).message}</p>
  {/snippet}
</svelte:boundary>
