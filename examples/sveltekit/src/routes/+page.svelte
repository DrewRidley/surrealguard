<!--
  The demo. One page, three things.

  1. THE QUERY IS IN THE MARKUP. The `q=` attribute below is written where you
     are looking when you want to change it, it is typed from your schema, and
     SurrealGuard reports a mistake ON THAT LINE. Move the slider and it
     re-runs; press a button and the rows arrive over a live subscription.

     `{minAge}` is NOT string interpolation. Svelte would compile an
     interpolated attribute to concatenation, which would splice the value into
     the query text — a SurrealQL injection for a string, and a brand-new query
     text (so a brand-new cache entry) on every keystroke for a number. The
     preprocessor in `@surrealguard/svelte/preprocess` catches the attribute
     before the compiler and captures the parts, so what actually runs is one
     text with a real bound parameter:

       SELECT … WHERE age > $__host0        { __host0: minAge }

     One query text, whatever the slider says.

  2. THE PAGE KEEPS NO DATA. There is no array of people, no list of teams, no
     counter — every row on screen, including the options in the "add a person"
     dropdown, is a <Query> against the schema. The only `$state` here is what
     the human is typing: the slider, and the three form fields.

  3. WHO YOU ARE CHANGES WHAT YOU SEE. The ticket query below never changes.
     `PERMISSIONS FOR select WHERE team = $auth.team` on the table, and
     `DEFINE ACCESS staff … TYPE RECORD` next to it — both in
     `schema/schema.surql` — do all of it. There is no authorisation logic in
     this app.

  Break the query in the attribute below to see it: `persn` for `person` gives,
  on that line and under that word,

    error[E1001]: `persn` is not a defined table
      help: did you mean `person`?

  Three more, with their exact messages, are at the bottom of
  `src/lib/queries.ts`.
-->
<script lang="ts">
  import { recordId } from "@surrealguard/client";
  import { createMutation, LiveQuery, Query } from "@surrealguard/svelte";
  import { addPerson, removePerson } from "$lib/queries";
  import type { PersonRows, TeamRows, TicketRows } from "$lib/inline-registry";
  import { LOGINS, session } from "$lib/session.svelte";

  // Plain `$state`. Nothing about it knows there is a database.
  let minAge = $state(25);

  // The add form's fields — what the human is typing, and nothing else. No row
  // the database owns is mirrored here; the teams in the picker are a <Query>.
  //
  // `team` is the id of a `team` record, spelled the way JSON spells it, and
  // that type is DERIVED: it is the `id` column of the very query the picker
  // runs. Add a team to the database and the picker offers it; there is no list
  // to update.
  let name = $state("");
  let age = $state(30);
  let team = $state<TeamRows[number]["id"] | "">("");

  const add = createMutation(addPerson, { onSuccess: () => (name = "") });
  const remove = createMutation(removePerson);
</script>

<header>
  <h1>SurrealGuard <span class="thin">— SvelteKit</span></h1>
  <div class="viewer">
    <span class="label">signed in as</span>
    <strong>{session.viewer.kind === "root" ? "root" : session.viewer.name}</strong>
  </div>
</header>

<section>
  <div class="control">
    <label>
      age &gt;
      <input type="range" min="20" max="50" bind:value={minAge} />
      <output data-testid="min-age">{minAge}</output>
    </label>
  </div>

  <div class="control">
    <input placeholder="name" bind:value={name} data-testid="new-name" />
    <input type="number" min="18" max="99" bind:value={age} data-testid="new-age" />

    <!-- The picker is a query. The options are rows of `team`, so there is no
         list of teams in this file to fall out of date. -->
    <Query q="SELECT id, name FROM team">
      {#snippet loading()}<span class="muted">teams…</span>{/snippet}
      {#snippet error(cause)}<span class="error">{cause.message}</span>{/snippet}
      {#snippet children(teams: TeamRows)}
        <select bind:value={team} data-testid="new-team">
          <option value="" disabled>pick a team</option>
          {#each teams as t (t.id)}
            <option value={t.id}>{t.name}</option>
          {/each}
        </select>
      {/snippet}
    </Query>

    <!-- A `<select>` hands back a string, and `team` on `person` is a
         `record<team>`: a plain string encodes to a SurrealQL string, which is
         a different type on the wire and a different value in the database.
         `recordId()` rebuilds the `RecordId` and keeps the inference — no
         cast, and the generated params type is what insists. The `team &&` is
         the same "nothing picked yet" the `disabled` says, spelled so the type
         checker can read it too. -->
    <button
      data-testid="add"
      disabled={add.pending || !name || !team}
      onclick={() => team && add.mutate({ name, age, team: recordId(team) })}
    >
      Add a person
    </button>
  </div>

  <LiveQuery q="SELECT id, name, age, team FROM person WHERE age > {minAge}">
    {#snippet loading()}<p class="muted">Subscribing…</p>{/snippet}
    {#snippet error(cause)}
      <p class="error">{cause.message}</p>
      <p class="muted">Is SurrealDB running? <code>pnpm db</code>, in another terminal.</p>
    {/snippet}
    <!-- The annotation is a stopgap, and `$lib/inline-registry.ts` explains
         exactly why: `svelte2tsx` type-checks the ORIGINAL markup, so it sees
         a string here rather than the query the preprocessor makes of it. The
         type is still DERIVED from the schema — `person.age` is a `number`,
         `person.nope` is a compile error — and both go away when markup
         extraction lands. -->
    {#snippet children(people: PersonRows)}
      <ul data-testid="people" class="rows">
        {#each people as person (person.id)}
          <li>
            <strong>{person.name}</strong>
            <span class="muted">{person.age} · {person.team}</span>
            <button
              class="x"
              title="delete {person.name}"
              onclick={() => remove.mutate({ person: recordId(person.id) })}>×</button
            >
          </li>
        {/each}
      </ul>
      <p class="count" data-testid="people-count">{people.length} people</p>
    {/snippet}
  </LiveQuery>
  {#if add.error}<p class="error">{add.error.message}</p>{/if}
  {#if remove.error}<p class="error">{remove.error.message}</p>{/if}
</section>

<section>
  <div class="control">
    {#each LOGINS as login (login.email)}
      <button onclick={() => session.signIn(login)} disabled={session.busy}>
        {login.name} ({login.team})
      </button>
    {/each}
    <button onclick={() => session.signOut()} disabled={session.busy}>root</button>
    {#if session.error}<span class="error">{session.error}</span>{/if}
  </div>

  <Query q="SELECT id, title, team FROM ticket">
    {#snippet loading()}<p class="muted">Loading tickets…</p>{/snippet}
    {#snippet error(cause, retry)}
      <p class="error">{cause.message}</p>
      <button onclick={retry}>Retry</button>
    {/snippet}
    {#snippet children(tickets: TicketRows)}
      <ul data-testid="tickets" class="rows">
        {#each tickets as ticket (ticket.id)}
          <li><strong>{ticket.title}</strong> <span class="muted">{ticket.team}</span></li>
        {:else}
          <li class="muted">no tickets visible to this identity</li>
        {/each}
      </ul>
      <p class="count" data-testid="ticket-count">{tickets.length} tickets visible</p>
    {/snippet}
  </Query>
</section>

<style>
  header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 1rem;
    flex-wrap: wrap;
  }
  h1 {
    margin: 0;
    font-size: 1.6rem;
  }
  .thin {
    font-weight: 400;
    color: #6b7280;
  }
  .viewer .label {
    color: #6b7280;
  }
  section {
    margin: 2rem 0;
    padding-top: 1.25rem;
    border-top: 1px solid #e5e7eb;
  }
  .control {
    display: flex;
    align-items: center;
    gap: 1rem;
    flex-wrap: wrap;
    margin-bottom: 1rem;
  }
  label {
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }
  input[type="range"] {
    width: 14rem;
  }
  .control input:not([type="range"]),
  .control select {
    font: inherit;
    padding: 0.3rem 0.5rem;
    border: 1px solid #d1d5db;
    border-radius: 6px;
    background: white;
  }
  .control input[type="number"] {
    width: 5rem;
  }
  output {
    font-variant-numeric: tabular-nums;
    font-weight: 600;
    min-width: 2ch;
  }
  .rows {
    list-style: none;
    padding: 0;
    margin: 0.5rem 0;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  .rows li {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.35rem 0.6rem;
    border-radius: 6px;
    background: #f9fafb;
  }
  .muted {
    color: #6b7280;
    font-size: 0.92rem;
  }
  .count {
    color: #6b7280;
    font-size: 0.85rem;
    margin: 0.35rem 0 0;
  }
  .error {
    color: #b91c1c;
  }
  button {
    font: inherit;
    padding: 0.35rem 0.75rem;
    border: 1px solid #d1d5db;
    border-radius: 6px;
    background: white;
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  button.x {
    margin-left: auto;
    border: none;
    background: transparent;
    color: #9ca3af;
    padding: 0 0.35rem;
  }
  button.x:hover {
    color: #b91c1c;
  }
</style>
