/**
 * Next.js / React bindings.
 *
 * Server components fetch typed data with `client.query(...)` and pass it as
 * `initialData`; a client component's {@link useLiveQuery} hook seeds from that
 * data (no hydration gap), subscribes on mount, reconciles live notifications,
 * and releases the (reference-counted) subscription on unmount.
 */

import { useMemo, useSyncExternalStore } from "react";
import type { QueryClient, QueryState } from "@surrealguard/query";

export interface UseLiveQueryOptions<Row> {
  params?: Record<string, unknown>;
  initialData?: Row[];
}

/**
 * Subscribe to a query as reactive state. `LIVE SELECT …` keeps updating;
 * a plain `SELECT …` resolves once. The subscription is torn down on unmount.
 */
export function useLiveQuery<Row extends Record<string, unknown> = Record<string, unknown>>(
  client: QueryClient,
  sql: string,
  options: UseLiveQueryOptions<Row> = {},
): QueryState<Row> {
  const paramsKey = options.params ? JSON.stringify(options.params) : "";
  const observable = useMemo(
    () => client.observe<Row>(sql, { params: options.params, initialData: options.initialData }),
    // Re-observe only when the query or its params change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [client, sql, paramsKey],
  );

  return useSyncExternalStore(
    observable.subscribe,
    observable.get,
    observable.get, // server snapshot: the seeded/initial state
  );
}
