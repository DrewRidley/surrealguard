<!--
  Provide the typed client once, at the root. Every `createQuery` / `createLive`
  below resolves it from context, so components never thread `db` through props.
-->
<script lang="ts">
  import { setClient } from "@surrealguard/svelte";
  import { db } from "$lib/db";

  setClient(db);

  let { children } = $props();
</script>

{@render children()}
