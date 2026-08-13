<!--
  The demo. Four beats, top to bottom, in the order they are meant to be shown.

    1. Reactive parameters — move the slider, the query re-runs.
    2. Live updates — add or remove a person, every list on the page follows.
    3. Nesting — one live subscription per row, parameterised by that row.
    4. Record-level access control — sign in, and the SAME query returns
       different rows, because SurrealDB's `PERMISSIONS` say so.

  Everything on this page is typed from `schema/schema.surql` through
  `src/lib/surrealguard.generated.ts`. No snippet parameter below carries a type
  annotation and every one of them is fully typed: `person.name` is a `string`,
  `person.nope` is a compile error, `team` is a `RecordId<"team">` in a parameter
  and the string `"team:red"` in a row.

  THE EDITOR MOMENT lives in `src/lib/queries.ts`, at the bottom: five
  commented-out one-liners, each with the exact diagnostic it produces.

  The quickest one to do from THIS file is a misspelled field. In beat 1 below,
  change `person.age` to `person.aeg` — that is TypeScript, not SurrealGuard,
  and it is a compile error because the row type came out of the schema.
-->
<script lang="ts">
  import { recordId } from "@surrealguard/client";
  import { createMutation, LiveQuery, Query } from "@surrealguard/svelte";
  import { RecordId } from "$lib/surrealguard.generated";
  import {
    addPerson,
    allPeople,
    allTeams,
    allTickets,
    livePeople,
    liveRoster,
    peopleOver,
    removePerson,
  } from "$lib/queries";
  import { LOGINS, session } from "$lib/session.svelte";

  // ---- Beat 1: a reactive parameter ------------------------------------
  // Plain `$state`. Nothing about it knows there is a database.
  let minAge = $state(30);

  // ---- Beat 2: writes --------------------------------------------------
  // `invalidates` is what tells the cache the one-shot queries above are now
  // wrong. The live queries reconcile on their own — they are listed anyway,
  // because a write that misses one is the bug this option exists to prevent
  // and there is no cost to naming them.
  const stale = [allPeople, allTeams, peopleOver, livePeople, liveRoster];

  const add = createMutation(addPerson, { invalidates: stale });
  const remove = createMutation(removePerson, { invalidates: stale });

  const CANDIDATES = [
    { name: "Edsger", age: 42, team: "red" },
    { name: "Radia", age: 38, team: "blue" },
    { name: "Margaret", age: 33, team: "red" },
    { name: "Barbara J", age: 47, team: "blue" },
  ] as const;
  let next = $state(0);

  function addOne() {
    const who = CANDIDATES[next % CANDIDATES.length]!;
    next += 1;
    // `team` must be a `RecordId`, not the string "team:red" — a plain string
    // encodes to a SurrealQL string and would match nothing. The type says so.
    add.mutate({ name: who.name, age: who.age, team: new RecordId("team", who.team) });
  }
</script>

<header>
  <h1>SurrealGuard <span class="thin">— SvelteKit</span></h1>
  <div class="viewer">
    <span class="label">connected as</span>
    <strong>
      {session.viewer.kind === "root" ? "root (system user)" : session.viewer.name}
    </strong>
  </div>
</header>

<!-- ==================================================================== -->
<section>
  <h2><span class="beat">1</span> A reactive parameter</h2>
  <p class="note">
    <code>minAge</code> is <code>$state</code>. It reaches the query through a
    <em>thunk</em> — <code>{"q={() => peopleOver.with({ minAge })}"}</code> — and that is the
    whole mechanism: the thunk re-runs when <code>minAge</code> changes, the query key
    changes with it, and the old result is dropped.
  </p>

  <label class="control">
    minimum age
    <input type="range" min="25" max="50" bind:value={minAge} />
    <output data-testid="min-age">{minAge}</output>
  </label>

  <Query q={() => peopleOver.with({ minAge })}>
    {#snippet loading()}<p class="muted">Loading…</p>{/snippet}
    {#snippet error(cause, retry)}
      <p class="error">{cause.message}</p>
      <button onclick={retry}>Retry</button>
    {/snippet}
    {#snippet children(people)}
      <ul data-testid="over" class="rows">
        {#each people as person (person.id)}
          <li><strong>{person.name}</strong> <span class="muted">{person.age}</span></li>
        {:else}
          <li class="muted">nobody that old</li>
        {/each}
      </ul>
      <p class="count" data-testid="over-count">{people.length} match</p>
    {/snippet}
  </Query>
</section>

<!-- ==================================================================== -->
<section>
  <h2><span class="beat">2</span> Live updates</h2>
  <p class="note">
    A <code>&lt;LiveQuery&gt;</code>. The buttons write to the database from this page — no
    second terminal — and the rows arrive over the subscription, not from the click. Open
    a second tab and both follow.
  </p>

  <div class="control">
    <button onclick={addOne} disabled={add.pending}>Add a person</button>
    {#if add.error}<span class="error">{add.error.message}</span>{/if}
    {#if remove.error}<span class="error">{remove.error.message}</span>{/if}
  </div>

  <LiveQuery q={livePeople}>
    {#snippet loading()}<p class="muted">Subscribing…</p>{/snippet}
    <!-- Give every LiveQuery an `error` snippet. Without one the failure is
         THROWN, which is the right default for a real app (it reaches
         `<svelte:boundary>` or SvelteKit's error page rather than leaving a
         page silently blank) — but in a demo it replaces the whole page with a
         stack trace, and "the database is not running" deserves better. -->
    {#snippet error(cause)}
      <p class="error">{cause.message}</p>
      <p class="muted">Is SurrealDB running? <code>pnpm db</code>, in another terminal.</p>
    {/snippet}
    {#snippet children(people)}
      <ul data-testid="live-people" class="rows">
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
      <p class="count" data-testid="live-count">{people.length} people</p>
    {/snippet}
  </LiveQuery>
</section>

<!-- ==================================================================== -->
<section>
  <h2><span class="beat">3</span> One subscription per row</h2>
  <p class="note">
    <code>&lt;Query&gt;</code> over the teams, and a <code>&lt;LiveQuery&gt;</code> mounted
    <em>inside</em> the <code>{"{#each}"}</code>, parameterised by the row it belongs to.
    Rows sharing a key share one <code>LIVE SELECT</code>; unmounting a row
    <code>KILL</code>s its subscription. Add someone above and the right team grows.
  </p>

  <Query q={allTeams}>
    {#snippet loading()}<p class="muted">Loading teams…</p>{/snippet}
    {#snippet children(teams)}
      <div class="teams">
        {#each teams as team (team.id)}
          <div class="team">
            <h3>{team.name}</h3>
            <!-- `team.id` is the JSON link "team:red"; a record PARAMETER has to
                 be the SDK's RecordId, so `recordId` rebuilds it. It infers
                 RecordId<"team"> from the literal type — no cast. -->
            <LiveQuery q={liveRoster.with({ team: recordId(team.id) })}>
              {#snippet loading()}<p class="muted">…</p>{/snippet}
              {#snippet error(cause)}<p class="error">{cause.message}</p>{/snippet}
              {#snippet children(members)}
                <ul class="rows" data-testid="roster-{team.id}">
                  {#each members as member (member.id)}
                    <li><strong>{member.name}</strong> <span class="muted">{member.age}</span></li>
                  {/each}
                </ul>
                <p class="count">{members.length} on {team.name}</p>
              {/snippet}
            </LiveQuery>
          </div>
        {/each}
      </div>
    {/snippet}
  </Query>
</section>

<!-- ==================================================================== -->
<section>
  <h2><span class="beat">4</span> Record-level access control</h2>
  <p class="note">
    <code>DEFINE ACCESS staff ON DATABASE TYPE RECORD</code> in
    <code>schema/schema.surql</code> owns the sign-in; the app has no auth logic. The
    query below is <code>SELECT id, title, done, team FROM ticket</code> — the same text
    every time. What changes is <code>$auth</code>, and
    <code>PERMISSIONS FOR select WHERE team = $auth.team</code> on the table does the rest.
  </p>

  <div class="control">
    {#each LOGINS as login (login.email)}
      <button onclick={() => session.signIn(login)} disabled={session.busy}>
        Sign in as {login.name} ({login.team})
      </button>
    {/each}
    <button onclick={() => session.signOut()} disabled={session.busy}>
      Sign out (back to root)
    </button>
    {#if session.error}<span class="error">{session.error}</span>{/if}
  </div>

  <Query q={allTickets}>
    {#snippet loading()}<p class="muted">Loading tickets…</p>{/snippet}
    {#snippet error(cause, retry)}
      <p class="error">{cause.message}</p>
      <button onclick={retry}>Retry</button>
    {/snippet}
    {#snippet children(tickets)}
      <ul data-testid="tickets" class="rows">
        {#each tickets as ticket (ticket.id)}
          <li>
            <strong class:done={ticket.done}>{ticket.title}</strong>
            <span class="muted">{ticket.team}</span>
          </li>
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
    font-size: 1.5rem;
  }
  .thin {
    font-weight: 400;
    color: #6b7280;
  }
  .viewer {
    font-size: 0.95rem;
  }
  .viewer .label {
    color: #6b7280;
  }
  section {
    margin: 2rem 0;
    padding-top: 1.25rem;
    border-top: 1px solid #e5e7eb;
  }
  h2 {
    font-size: 1.1rem;
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }
  .beat {
    display: inline-grid;
    place-items: center;
    width: 1.6rem;
    height: 1.6rem;
    border-radius: 999px;
    background: #111827;
    color: white;
    font-size: 0.85rem;
  }
  .note {
    color: #4b5563;
    max-width: 62ch;
    line-height: 1.55;
  }
  .control {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    flex-wrap: wrap;
    margin: 0.75rem 0;
  }
  input[type="range"] {
    width: 16rem;
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
    gap: 0.15rem;
  }
  .rows li {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.3rem 0.5rem;
    border-radius: 6px;
    background: #f9fafb;
  }
  .muted {
    color: #6b7280;
    font-size: 0.9rem;
  }
  .count {
    color: #6b7280;
    font-size: 0.85rem;
    margin: 0.25rem 0 0;
  }
  .error {
    color: #b91c1c;
  }
  .done {
    text-decoration: line-through;
    color: #6b7280;
  }
  .teams {
    display: flex;
    gap: 1.5rem;
    flex-wrap: wrap;
  }
  .team {
    flex: 1 1 16rem;
  }
  .team h3 {
    margin: 0.25rem 0;
    font-size: 1rem;
  }
  button {
    font: inherit;
    padding: 0.35rem 0.7rem;
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
