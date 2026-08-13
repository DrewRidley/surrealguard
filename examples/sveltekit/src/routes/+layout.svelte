<!--
  `setClient` is not the price of entry. `db.query`, `db.run` and `db.watch`
  work from a plain `import { db } from "$lib/db"` anywhere. This one call
  exists so the REACTIVE helpers — `<Query>`, `<LiveQuery>`, `createMutation` —
  can find a client without every component threading `db` through props. Each
  of them also takes `{ client: db }` directly.
-->
<script lang="ts">
  import "../app.css";
  import { setClient } from "@surrealguard/svelte";
  import { db, HEALTH_URL } from "$lib/db";

  setClient(db);

  let { children } = $props();

  // Demo scaffolding, and worth its ten lines: `Surreal.connect()` does not
  // reject when nothing is listening — it waits — so a page opened before
  // `pnpm db` shows a spinner forever and produces no error for any `error`
  // snippet to render. Asking over HTTP is the only way to say the useful
  // thing.
  let up = $state<boolean | undefined>(undefined);
  let startedDown = $state(false);

  $effect(() => {
    let stopped = false;
    let first = true;
    const tick = async () => {
      const ok = await fetch(HEALTH_URL, { cache: "no-store" }).then(
        (response) => response.ok,
        () => false,
      );
      if (stopped) return;
      if (first) {
        first = false;
        startedDown = !ok;
      }
      up = ok;
    };
    void tick();
    const timer = setInterval(() => void tick(), 1500);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  });
</script>

{#if up === false}
  <p class="banner">
    <strong>SurrealDB is not running.</strong>
    Start it with <code>pnpm db</code> in <code>examples/sveltekit</code>.
  </p>
{:else if up && startedDown}
  <!-- A dropped socket reconnects on its own; one that never opened does not. -->
  <p class="banner up">
    <strong>SurrealDB is up now.</strong>
    This page connected before it was, so it needs a reload.
    <button onclick={() => location.reload()}>Reload</button>
  </p>
{/if}

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
    max-width: 52rem;
    margin: 0 auto;
    padding: 2rem 1.5rem;
  }
</style>
