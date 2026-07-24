<!--
  Provide the typed client once, at the root. Every `liveQuery` below resolves
  it from context. (Illustrative Svelte 5 component — the type proof runs
  through the `.ts` files via `tsc`; check `.svelte` files with `svelte-check`.)
-->
<script lang="ts">
  import { setClient } from "@surrealguard/svelte";
  import { db } from "$lib/db";

  setClient(db);

  let { children } = $props();
</script>

{@render children()}
