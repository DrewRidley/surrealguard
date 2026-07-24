/**
 * SSR `load` helpers. A `LIVE SELECT` cannot resolve rows in one shot, so on the
 * server we run its underlying `SELECT` once and seed the cache under the live
 * key. Pass the result to {@link liveQuery}'s `initial` for a gap-free first
 * render that then upgrades to live on the client.
 *
 * ```ts
 * // +page.ts (or +page.server.ts)
 * import { loadLive } from "@surrealguard/svelte";
 * import { db } from "$lib/db";
 * export async function load() {
 *   const users = await loadLive(db, db.live(`SELECT * FROM user`));
 *   return { users };
 * }
 * ```
 * ```svelte
 * <!-- +page.svelte -->
 * <script>
 *   import { liveQuery } from "@surrealguard/svelte";
 *   let { data } = $props();
 *   const users = liveQuery((db) => db.live(`SELECT * FROM user`), { initial: data.users });
 * </script>
 * {#each users.data as user (user.id)}<li>{user.name}</li>{/each}
 * ```
 *
 * For whole-cache transport, {@link dehydrate} on the server and {@link hydrate}
 * on the client instead.
 */

import type { LiveDescriptor, SurrealGuardClient } from "@surrealguard/client";
import { getQueryClient, type DehydratedState } from "@surrealguard/query";

/**
 * Run a live descriptor's underlying `SELECT` once and return the typed rows,
 * caching them under the live key so a later {@link liveQuery} of the same query
 * renders without a refetch, then upgrades to live.
 */
export function loadLive<Row>(
  client: SurrealGuardClient,
  descriptor: LiveDescriptor<Row>,
  params?: Record<string, unknown>,
): Promise<Row[]> {
  return getQueryClient(client).prime(descriptor, params);
}

/** Snapshot a client's cached results for transport to the browser (SSR). */
export function dehydrate(client: SurrealGuardClient): DehydratedState {
  return getQueryClient(client).dehydrate();
}

/** Seed a client's cache from a server snapshot so the browser avoids a refetch. */
export function hydrate(client: SurrealGuardClient, state: DehydratedState): void {
  getQueryClient(client).hydrate(state);
}
