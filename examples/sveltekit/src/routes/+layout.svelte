<!--
  `setClient` is NOT the price of entry, and the docs used to read as if it
  were. `db.query`, `db.run` and `db.watch` work from a plain
  `import { db } from "$lib/db"` anywhere — a component, a `load`, a server
  route, a plain `.ts` module. Nothing has to be provided or wired up first.

  This call exists for exactly one thing: the REACTIVE helpers (`createQuery`,
  `createLive`, `createMutation`) resolve their client from Svelte context, so
  that components do not thread `db` through props. Every one of them also
  accepts `{ client: db }` directly — which is what a `.svelte.ts` module must
  do anyway, since `setContext` is only readable during component
  initialisation.

  So: using `createLive` in components? Call this once, here. Otherwise delete
  it.
-->
<script lang="ts">
  import { setClient } from "@surrealguard/svelte";
  import { db } from "$lib/db";
  import { health } from "$lib/health.svelte";

  setClient(db);

  let { children } = $props();

  // Demo scaffolding — see `$lib/health.svelte.ts`. Not something an app needs.
  $effect(() => health.watch());
</script>

{#if health.reachable === false}
  <p class="banner">
    <strong>SurrealDB is not running.</strong>
    Start it with <code>pnpm db</code> in <code>examples/sveltekit</code>, then reload.
  </p>
{:else if health.needsReload}
  <p class="banner up">
    <strong>SurrealDB is up now.</strong>
    This page connected before it was, so it needs a reload.
    <button onclick={() => location.reload()}>Reload</button>
  </p>
{/if}

<nav>
  <a href="/">demo</a>
  <a href="/plain">db.query</a>
  <a href="/live">createLive + preload</a>
  <a href="/components">&lt;Query&gt; / &lt;LiveQuery&gt;</a>
</nav>

<main>
  {@render children()}
</main>

<style>
  :global(body) {
    margin: 0;
    font-family:
      ui-sans-serif,
      system-ui,
      -apple-system,
      "Segoe UI",
      sans-serif;
    color: #111827;
    background: white;
  }
  :global(code) {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.9em;
    background: #f3f4f6;
    padding: 0.1em 0.35em;
    border-radius: 4px;
  }
  nav {
    display: flex;
    gap: 1.25rem;
    padding: 0.75rem 1.5rem;
    border-bottom: 1px solid #e5e7eb;
    font-size: 0.9rem;
  }
  nav a {
    color: #2563eb;
    text-decoration: none;
  }
  nav a:hover {
    text-decoration: underline;
  }
  .banner {
    margin: 0;
    padding: 0.7rem 1.5rem;
    background: #fef2f2;
    border-bottom: 1px solid #fecaca;
    color: #991b1b;
    font-size: 0.9rem;
  }
  .banner.up {
    background: #f0fdf4;
    border-bottom-color: #bbf7d0;
    color: #166534;
  }
  .banner button {
    font: inherit;
    margin-left: 0.5rem;
    padding: 0.15rem 0.6rem;
    border: 1px solid currentColor;
    border-radius: 5px;
    background: transparent;
    color: inherit;
    cursor: pointer;
  }
  main {
    max-width: 56rem;
    margin: 0 auto;
    padding: 1.5rem;
  }
</style>
