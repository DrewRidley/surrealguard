"use client";

/**
 * `useLiveQuery` — a React hook over the reactive core, backed by
 * `useSyncExternalStore`. Returns `{ data, status, error }`; a `LIVE SELECT …`
 * keeps `data` reconciled as notifications arrive, a plain `SELECT …` resolves
 * once. The (reference-counted) subscription is released on unmount.
 *
 * ```tsx
 * "use client";
 * import { useLiveQuery } from "@surrealguard/next";
 * export function Users() {
 *   const { data } = useLiveQuery((db) => db.live(`SELECT * FROM user`));
 *   return <ul>{data.map((u) => <li key={String(u.id)}>{u.name}</li>)}</ul>;
 * }
 * ```
 */

import { useMemo, useSyncExternalStore } from "react";
import type { LiveDescriptor, SurrealGuardClient } from "@surrealguard/client";
import { getQueryClient, type QueryState } from "@surrealguard/query";
import { useClient } from "./context.js";

export interface UseLiveQueryOptions<Row> {
  /** Override the context client (e.g. tests, multiple connections). */
  client?: SurrealGuardClient;
  /** Bindings for the live query. */
  params?: Record<string, unknown>;
  /** Seed rows (e.g. from a server fetch via `queryServer`) for a gap-free first render. */
  initialData?: Row[];
}

/**
 * Subscribe to `db.live(...)` as reactive state. The callback receives the
 * client (from context unless `options.client` overrides) and returns a typed
 * {@link LiveDescriptor}; `Row` flows through so `data` is `Row[]`.
 */
export function useLiveQuery<Row>(
  fn: (db: SurrealGuardClient) => LiveDescriptor<Row>,
  options: UseLiveQueryOptions<Row> = {},
): QueryState<Row> {
  const client = useClient(options.client);
  const descriptor = fn(client);
  const paramsKey = options.params ? JSON.stringify(options.params) : "";

  const observable = useMemo(
    () =>
      getQueryClient(client).observeLive(descriptor, {
        params: options.params,
        initialData: options.initialData,
      }),
    // Re-observe only when the query text, its params, or the client change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [client, descriptor.sql, paramsKey],
  );

  return useSyncExternalStore(observable.subscribe, observable.get, observable.get);
}
