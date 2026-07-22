/**
 * Svelte / SvelteKit bindings.
 *
 * A `+page.server.ts` `load` fetches typed data with `client.query(...)` and
 * returns it; the page seeds {@link liveQuery} with `initial`, which subscribes
 * on mount, reconciles live notifications, and unsubscribes on destroy — Svelte
 * stores tear down automatically when the last subscriber leaves. Usage:
 *
 * ```svelte
 * <script>
 *   export let data;
 *   const users = liveQuery(qc, "LIVE SELECT * FROM user", { initial: data.users });
 * </script>
 * {#each $users.data as user}<li>{user.name}</li>{/each}
 * ```
 */

import { readable, type Readable } from "svelte/store";
import type { QueryClient, QueryState } from "@surrealguard/query";

export interface LiveQueryOptions<Row> {
  params?: Record<string, unknown>;
  /** Seed data (e.g. from a `load` function) for a gap-free first render. */
  initial?: Row[];
}

/**
 * A readable store of query state. `LIVE SELECT …` keeps updating; a plain
 * `SELECT …` resolves once. The underlying subscription is reference-counted
 * and released when the store loses its last subscriber.
 */
export function liveQuery<Row extends Record<string, unknown> = Record<string, unknown>>(
  client: QueryClient,
  sql: string,
  options: LiveQueryOptions<Row> = {},
): Readable<QueryState<Row>> {
  const observable = client.observe<Row>(sql, {
    params: options.params,
    initialData: options.initial,
  });
  return readable(observable.get(), (set) => observable.subscribe(set));
}
