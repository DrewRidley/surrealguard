<!--
  Level 3: SSR + live, and what `defineLive` is actually for.

  Note what is NOT here: the query text. `+page.ts` ran `livePeople` and the
  payload it returned carries its own key, so this component subscribes to
  exactly that query without naming it a second time. That is the reason a
  named query value exists — two places needing to agree on one query — and it
  is the only reason. `db.query` on the previous page needed none of it.

  `people.data` is reactive state read directly, with no store `$` prefix. It
  renders from the SSR seed on the first paint and upgrades to live.

  `createLive` resolves the client from context, which is why `+layout.svelte`
  calls `setClient(db)`. Pass `{ client: db }` instead and the context call is
  unnecessary — see `$lib/people.svelte.ts`.
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
