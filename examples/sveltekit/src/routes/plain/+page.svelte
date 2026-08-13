<!--
  Level 1: the whole thing, with nothing around it.

  No `defineQuery`, no `setClient`, no `+page.ts`. Import the client, write the
  SurrealQL you already know, and the rows are typed from your schema —
  `person.name` is a `string`, `person.team` is a `RecordId<"team">`, and
  `person.nope` is a compile error.

  Two things this file cannot do without. Both fail SILENTLY — every field
  becomes `any` and nothing complains — so they are worth saying out loud:

    1. `lang="ts"` on the script tag. Without it Svelte does not typecheck the
       block at all.
    2. `@surrealguard/client` actually installed. The generated module augments
       that module BY NAME; if the name does not resolve, TypeScript drops the
       whole `declare module` block (TS2664) and every lookup falls back to
       `any` — with the error landing in the generated file, which you would
       never open.
-->
<script lang="ts">
  import { db } from "$lib/db";

  // SurrealDB returns one result per statement, so `query` resolves the
  // per-statement tuple. One statement, one element.
  const rows = db.query("SELECT id, name, age, team FROM person");
</script>

<h1>People</h1>

{#await rows}
  <p>Loading…</p>
{:then [people]}
  <ul>
    {#each people as person (person.id)}
      <li>{person.name} — {person.age} — {person.team.id}</li>
    {/each}
  </ul>
{:catch error}
  <p class="error">{error.message}</p>
{/await}

<p><a href="/live">The same rows, preloaded on the server and live →</a></p>
