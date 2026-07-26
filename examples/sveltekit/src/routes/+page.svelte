<!--
  The page. Note what is NOT here: the query text. `data.people` carries its own
  key, so this subscribes to exactly the query `+page.ts` ran — the seed cannot
  be silently discarded by a one-byte drift, because there is nothing to drift.

  `people.data` is reactive state read directly, with no store `$` prefix. It
  renders from the SSR seed on the first paint and upgrades to live.
-->
<script lang="ts">
  import { createLive } from "@surrealguard/svelte";
  import type { load } from "./+page";

  // A real SvelteKit app writes `import type { PageData } from "./$types"`,
  // which SvelteKit generates as exactly this type. Spelled out here so the
  // example type-checks without running `svelte-kit sync`.
  type PageData = Awaited<ReturnType<typeof load>>;

  let { data }: { data: PageData } = $props();

  // Through a thunk, so a client-side navigation that replaces `data` re-runs
  // this rather than pinning the first payload forever.
  const people = createLive(() => data.people);
</script>

<h1>People</h1>

{#if people.error}
  <p class="error">{people.error.message}</p>
{:else}
  <ul>
    {#each people.data as person (person.id)}
      <li>{person.name} — {person.age}</li>
    {/each}
  </ul>
{/if}
