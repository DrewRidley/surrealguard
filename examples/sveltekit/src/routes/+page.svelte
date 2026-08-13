<!--
  Every read on this page is written where you are looking at it, and every one
  of them is typed by `surrealguard generate` — no annotations below, anywhere.
  Change a field name in an attribute and the editor says so before the app runs.
-->
<script lang="ts">
  import { LiveQuery, Query } from "@surrealguard/svelte";
  import { recordId } from "@surrealguard/client";
  import { db } from "$lib/db";
  import { LOGINS, session } from "$lib/session.svelte";

  let minAge = $state(25);
  let name = $state("");
  let age = $state(30);
  let team = $state("");

  async function add() {
    if (!name || !team) return;
    await db.query("CREATE person SET name = $name, age = $age, team = $team", {
      name,
      age,
      team: recordId(team as `team:${string}`),
    });
    name = "";
  }

  const remove = (person: `person:${string}`) =>
    db.query("DELETE person WHERE id = $person", { person: recordId(person) });
</script>

<div class="mx-auto max-w-3xl space-y-8 p-8 text-slate-100">
  <section class="space-y-3">
    <label class="flex items-center gap-3">
      <span class="w-28 text-sm text-slate-400">age &gt; {minAge}</span>
      <input type="range" min="20" max="50" bind:value={minAge} class="flex-1" />
    </label>

    <div class="flex gap-2">
      <input
        placeholder="name"
        bind:value={name}
        data-testid="new-name"
        class="flex-1 rounded bg-slate-800 px-3 py-2"
      />
      <input
        type="number"
        min="18"
        max="99"
        bind:value={age}
        data-testid="new-age"
        class="w-20 rounded bg-slate-800 px-3 py-2"
      />

      <Query q="SELECT id, name FROM team">
        {#snippet children(teams)}
          <select bind:value={team} data-testid="new-team" class="rounded bg-slate-800 px-3 py-2">
            <option value="" disabled>team</option>
            {#each teams as t (t.id)}<option value={t.id}>{t.name}</option>{/each}
          </select>
        {/snippet}
      </Query>

      <button
        onclick={add}
        disabled={!name || !team}
        class="rounded bg-sky-600 px-4 py-2 font-medium disabled:opacity-40">Add</button
      >
    </div>
  </section>

  <!-- Reactive parameters: `params` re-runs this subscription as the slider moves. -->
  <LiveQuery q="SELECT id, name, age, team FROM person WHERE age > $min" params={{ min: minAge }}>
    {#snippet loading()}<p class="text-slate-500">…</p>{/snippet}
    {#snippet children(people)}
      <ul class="divide-y divide-slate-800 rounded border border-slate-800">
        {#each people as person (person.id)}
          <li class="flex items-center gap-3 px-4 py-2" data-testid="person-{person.name}">
            <span class="flex-1">{person.name}</span>
            <span class="text-slate-400">{person.age}</span>
            <button onclick={() => remove(person.id)} class="text-slate-500 hover:text-rose-400"
              >×</button
            >
          </li>
        {/each}
      </ul>
    {/snippet}
  </LiveQuery>

  <!-- Who you are decides what this returns: `ticket` PERMISSIONS read `$auth`. -->
  <section class="space-y-3">
    <div class="flex items-center gap-2 text-sm">
      {#each LOGINS as login (login.email)}
        <button
          onclick={() => session.signIn(login)}
          disabled={session.busy}
          class="rounded bg-slate-800 px-3 py-1 hover:bg-slate-700">{login.name}</button
        >
      {/each}
      <button
        onclick={() => session.signOut()}
        disabled={session.busy}
        class="rounded bg-slate-800 px-3 py-1 hover:bg-slate-700">root</button
      >
      <span class="text-slate-500">
        {session.viewer.kind === "root" ? "root — sees every ticket" : `${session.viewer.name} — ${session.viewer.team}`}
      </span>
    </div>

    <Query q="SELECT id, title, team FROM ticket">
      {#snippet children(tickets)}
        <ul class="grid grid-cols-2 gap-2" data-testid="tickets">
          {#each tickets as ticket (ticket.id)}
            <li class="rounded bg-slate-800/60 px-3 py-2 text-sm">{ticket.title}</li>
          {/each}
        </ul>
      {/snippet}
    </Query>
  </section>
</div>
