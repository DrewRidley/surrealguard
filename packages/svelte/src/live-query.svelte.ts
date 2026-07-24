/**
 * `liveQuery` — a Svelte 5 runes wrapper over the reactive core.
 *
 * The returned object is `$state`-backed: read `users.data` / `users.status` /
 * `users.loading` / `users.error` DIRECTLY in markup, no store `$` prefix. A
 * `LIVE SELECT …` keeps `data` reconciled as notifications arrive; a plain
 * `SELECT …` resolves once. The subscription starts in an `$effect` and is torn
 * down (reference-counted in the core) when the component is destroyed.
 *
 * ```svelte
 * <script>
 *   import { liveQuery } from "@surrealguard/svelte";
 *   const users = liveQuery((db) => db.live(`SELECT * FROM user`));
 * </script>
 * {#each users.data as user (user.id)}<li>{user.name}</li>{/each}
 * ```
 */

import type { LiveDescriptor, SurrealGuardClient } from "@surrealguard/client";
import { getQueryClient, type QueryStatus } from "@surrealguard/query";
import { getClient } from "./context.js";

export interface LiveQueryOptions<Row> {
  /** Override the context client (e.g. tests, multiple connections). */
  client?: SurrealGuardClient;
  /** Bindings for the live query. */
  params?: Record<string, unknown>;
  /** Seed rows (e.g. from a `load` via {@link loadLive}) for a gap-free first render. */
  initial?: Row[];
}

/** A runes-reactive view of a live query. Read its fields directly. */
export interface LiveQuery<Row> {
  readonly data: Row[];
  readonly status: QueryStatus;
  readonly loading: boolean;
  readonly error: unknown;
}

/**
 * Subscribe to `db.live(...)` as runes-reactive state. The callback receives the
 * client (from context unless `options.client` overrides) and returns a typed
 * {@link LiveDescriptor}; `Row` flows through so `users.data` is `Row[]`.
 */
export function liveQuery<Row>(
  fn: (db: SurrealGuardClient) => LiveDescriptor<Row>,
  options: LiveQueryOptions<Row> = {},
): LiveQuery<Row> {
  const client = options.client ?? getClient();
  const descriptor = fn(client);
  const observable = getQueryClient(client).observeLive(descriptor, {
    params: options.params,
    initialData: options.initial,
  });

  let state = $state(observable.get());

  $effect(() => {
    // subscribe() delivers the current state synchronously and starts the
    // (reference-counted) subscription; the returned unsubscribe runs on teardown.
    return observable.subscribe((next) => {
      state = next;
    });
  });

  return {
    get data() {
      return state.data;
    },
    get status() {
      return state.status;
    },
    get loading() {
      return state.status === "loading";
    },
    get error() {
      return state.error;
    },
  };
}
