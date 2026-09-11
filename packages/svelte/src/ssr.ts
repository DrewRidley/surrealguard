/**
 * SSR helpers.
 *
 * ```ts
 * // +page.ts
 * import { preload } from "@surrealdb/analyzer-svelte";
 * import { db } from "$lib/db";
 * import { livePeople } from "$lib/queries";
 *
 * export async function load() {
 *   return { people: await preload(db, livePeople) };
 * }
 * ```
 * ```svelte
 * <!-- +page.svelte : the query text appears nowhere, so the key cannot drift -->
 * <script lang="ts">
 *   import { createLive } from "@surrealdb/analyzer-svelte";
 *   let { data } = $props();
 *   const people = createLive(data.people);
 * </script>
 * ```
 *
 * The payload is plain values, so devalue accepts it without a `transport`
 * hook. If you would rather ship SDK class instances through `load`, see
 * `@surrealdb/analyzer-svelte/transport`.
 */

import { preload, type SurrealQLAnalyzerClient } from "@surrealdb/analyzer-client";
import { getQueryClient, type DehydratedState } from "@surrealdb/analyzer-query";

export { preload };

/** Snapshot a client's cached results for transport to the browser. */
export function dehydrate(client: SurrealQLAnalyzerClient): DehydratedState {
  return getQueryClient(client).dehydrate();
}

/** Seed a client's cache from a server snapshot so the browser avoids a refetch. */
export function hydrate(client: SurrealQLAnalyzerClient, state: DehydratedState): void {
  getQueryClient(client).hydrate(state);
}
