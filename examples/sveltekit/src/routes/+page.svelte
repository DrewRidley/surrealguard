<script lang="ts">
  import { recordId } from "@surrealguard/client";
  import { createMutation, LiveQuery, Query } from "@surrealguard/svelte";
  import { RecordId } from "$lib/surrealguard.generated";
  import { addPerson, removePerson } from "$lib/queries";
  import type { PersonRows, TicketRows } from "$lib/inline-registry";
  import { LOGINS, session } from "$lib/session.svelte";

  // Plain `$state`. Nothing about it knows there is a database.
  let minAge = $state(25);

  const add = createMutation(addPerson);
  const remove = createMutation(removePerson);

  const CANDIDATES = [
    { name: "Edsger", age: 42, team: "red" },
    { name: "Radia", age: 38, team: "blue" },
    { name: "Margaret", age: 33, team: "red" },
    { name: "Katsu", age: 47, team: "blue" },
  ] as const;
  let next = $state(0);

  function addOne() {
    const who = CANDIDATES[next % CANDIDATES.length]!;
    next += 1;
    // `team` has to be a `RecordId`, not the string "team:red": a plain string
    // encodes to a SurrealQL string and matches nothing. The type says so.
    add.mutate({ name: who.name, age: who.age, team: new RecordId("team", who.team) });
  }
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
    <button onclick={addOne} disabled={add.pending}>Add a person</button>
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
