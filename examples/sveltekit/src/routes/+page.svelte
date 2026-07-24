<!--
  The page. `liveQuery` returns runes-reactive state: read `people.data` /
  `people.loading` directly in markup. It seeds from the `load`'s rows (SSR) and
  upgrades to live on the client.

  Illustrative Svelte 5 component — the typed round-trip is proven by `tsc` over
  the `.ts` files (`src/lib/people.ts`, `src/routes/+page.ts`). To type-check the
  `.svelte` files too, run `svelte-check`.
-->
<script lang="ts">
  import { liveQuery } from "@surrealguard/svelte";
  import type { PageData } from "./$types";

  let { data }: { data: PageData } = $props();

  // Row type inferred from the generated registry; `data.people` seeds SSR.
  const people = liveQuery(
    (db) => db.live(`SELECT name, age, team FROM person`),
    { initial: data.people },
  );
</script>

<h1>People</h1>

{#if people.loading}
  <p>Loading…</p>
{:else}
  <ul>
    {#each people.data as person (person.team)}
      <li>{person.name} — {person.age}</li>
    {/each}
  </ul>
{/if}
