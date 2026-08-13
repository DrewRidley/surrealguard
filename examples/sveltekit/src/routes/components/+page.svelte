<!--
  Level 4: the same data with no runes primitive wired up by hand.

  `<Query>` and `<LiveQuery>` are thin wrappers over `createQuery` / `createLive`
  — same cache, same refcounting, same teardown — for when a route would rather
  say what it renders than assemble a handle first. Compare `/live`, which does
  this with `createLive` and an `{#if}`.

  Two things to notice, because they are the reason these exist:

  1. `people` and `teammates` below are written with NO type annotation, and
     both are fully typed: `person.name` is a `string`, `person.nope` is a
     compile error. The row type flows out of `q` and into the snippet.
  2. The inner `<LiveQuery>` is mounted PER ROW. That is the intended shape, not
     an abuse of it — a live subscription is cheap, and unmounting a row `KILL`s
     its subscription immediately, so a long list can subscribe to what is on
     screen and drop the rest. Rows sharing a team share one `LIVE SELECT`
     between them; the cache is refcounted by query key.
-->
<script lang="ts">
  import { LiveQuery, Query } from "@surrealguard/svelte";
  import { recordId } from "@surrealguard/client";
  import { liveRoster } from "$lib/queries";
  import type { load } from "./+page";

  // A real app writes `import type { PageData } from "./$types"`. Spelled out
  // so the example type-checks without running `svelte-kit sync`.
  type PageData = Awaited<ReturnType<typeof load>>;

  let { data }: { data: PageData } = $props();
</script>

<h1>People</h1>

<!-- Through a thunk, so a client-side navigation that replaces `data` re-runs
     this rather than pinning the first payload forever. -->
<Query q={() => data.people}>
  {#snippet loading()}
    <p>Loading…</p>
  {/snippet}

  {#snippet error(cause, retry)}
    <p class="error">{cause.message}</p>
    <button onclick={retry}>Retry</button>
  {/snippet}

  {#snippet children(people)}
    <ul>
      {#each people as person (person.id)}
        <li>
          {person.name} — {person.age}

          <!-- One live subscription per row, parameterised by a field of the
               outer row. `person.team` is the JSON link `"team:red"`; a record
               PARAMETER has to be the SDK's `RecordId`, so `recordId` rebuilds
               it. It ships in `@surrealguard/client`. -->
          <LiveQuery q={liveRoster.with({ team: recordId(person.team) })}>
            {#snippet loading()}
              <small>counting teammates…</small>
            {/snippet}
            {#snippet children(teammates)}
              <small>{teammates.length} on {person.team}</small>
            {/snippet}
          </LiveQuery>
        </li>
      {/each}
    </ul>
  {/snippet}
</Query>

<p><a href="/live">The same rows through `createLive` →</a></p>
