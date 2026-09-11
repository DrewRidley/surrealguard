"use client";

/**
 * React hooks over the reactive core, backed by `useSyncExternalStore` — the
 * correct primitive for an external store, and unchanged from 0.4.
 *
 * **These take the query reference directly — no thunk.** This is the one place
 * Svelte and React diverge, and they diverge because the frameworks do: a
 * `SurqlQuery` already carries a stable `key` string, so the hook's `useMemo`
 * dependency is `[client, source.key]` and React's "did my deps change" problem
 * simply does not arise. Svelte needs the thunk because its reactivity is
 * pull-based and the arguments are evaluated once.
 *
 * ```tsx
 * "use client";
 * import { useLive } from "@surrealdb/analyzer-next";
 * import { livePeople } from "@/lib/queries";
 *
 * export function People() {
 *   const people = useLive(livePeople);
 *   if (people.error) return <p>{people.error.message}</p>;
 *   return <ul>{people.data.map((p) => <li key={p.id}>{p.name}</li>)}</ul>;
 * }
 * ```
 */

import { useCallback, useMemo, useRef, useState, useSyncExternalStore } from "react";
import {
  SurrealQLAnalyzerError,
  type AnyQuery,
  type Bound,
  type Json,
  type ParamsArg,
  type Preloaded,
  type Rows,
  type SurqlLive,
  type SurqlQuery,
  type SurrealQLAnalyzerClient,
} from "@surrealdb/analyzer-client";
import { getQueryClient, type Observable, type QueryState } from "@surrealdb/analyzer-query";
import { useClient } from "./context.js";
import { fromPreloaded, isPreloaded } from "./preloaded.js";

/** "Not yet" — Convex's conditional-query sentinel, so a hook can opt out. */
export type Skip = "skip";

export interface UseQueryOptions {
  /** Override the context client (tests, multiple connections). */
  client?: SurrealQLAnalyzerClient;
}

export interface QueryResult<T> {
  readonly data: T | undefined;
  readonly error: SurrealQLAnalyzerError | undefined;
  readonly loading: boolean;
  readonly status: "pending" | "success" | "error";
}

export interface LiveResult<Row> {
  readonly data: Row[];
  readonly error: SurrealQLAnalyzerError | undefined;
  readonly loading: boolean;
  readonly status: "pending" | "success" | "error";
}

const PENDING: QueryState<unknown> = { status: "pending", data: undefined, error: undefined };
const NOOP = () => () => {};

/** Shared plumbing: memoise on the key, then `useSyncExternalStore`. */
function useObservedState(
  client: SurrealQLAnalyzerClient,
  source: SurqlQuery<unknown, Bound> | SurqlLive<unknown, Bound> | Preloaded<unknown> | Skip,
  live: boolean,
): QueryState<unknown> {
  const key = source === "skip" ? "skip" : source.key;

  const observable = useMemo<Observable<unknown> | undefined>(() => {
    if (source === "skip") return undefined;
    const core = getQueryClient(client);
    const preloadedSeed = isPreloaded(source) ? source.data : undefined;
    const query = isPreloaded(source) ? fromPreloaded(source) : source;
    return live
      ? (core.observeLive(query as SurqlLive<unknown, Bound>, {
          initialData: preloadedSeed as unknown[] | undefined,
        }) as Observable<unknown>)
      : (core.observe(query as SurqlQuery<unknown, Bound>, {
          initialData: preloadedSeed,
        }) as Observable<unknown>);
    // Re-observe only when the client or the query's own key changes. The key
    // is precomputed on the reference, so it cannot disagree with the cache.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, key, live]);

  const subscribe = observable?.subscribe ?? NOOP;
  const snapshot = useCallback(() => observable?.get() ?? PENDING, [observable]);
  return useSyncExternalStore(subscribe, snapshot, snapshot);
}

/**
 * A one-shot query. `data` is `T | undefined`, because a one-shot query's
 * result may be a scalar and there is nothing honest to default it to.
 */
export function useQuery<R>(
  source: SurqlQuery<R, Bound> | Preloaded<Json<Rows<R>>> | Skip,
  options: UseQueryOptions = {},
): QueryResult<Json<Rows<R>>> {
  const client = useClient(options.client);
  const state = useObservedState(client, source, false);
  return {
    data: state.data as Json<Rows<R>> | undefined,
    error: state.error,
    loading: state.status === "pending",
    status: state.status,
  };
}

/**
 * A live query. `data` is always an array — a live query is a row stream — so
 * `people.data.map(...)` needs no `?? []`.
 *
 * Passing a {@link Preloaded} payload hydrates from the server's rows and then
 * upgrades to live, with the query text appearing nowhere in the component.
 */
export function useLive<Row>(
  source: SurqlLive<Row, Bound> | Preloaded<Json<Row>[]> | Skip,
  options: UseQueryOptions = {},
): LiveResult<Json<Row>> {
  const client = useClient(options.client);
  const state = useObservedState(client, source, true);
  return {
    data: (state.data ?? []) as Json<Row>[],
    error: state.error,
    loading: state.status === "pending",
    status: state.status,
  };
}

export interface UseMutationOptions<R> {
  client?: SurrealQLAnalyzerClient;
  /**
   * Queries this write makes stale. Deliberately untyped as {@link AnyQuery}:
   * an invalidation target has no reason to agree with the mutation's own
   * result or parameter types.
   */
  invalidates?: readonly AnyQuery[];
  onSuccess?(data: Rows<R>): void;
  onError?(error: SurrealQLAnalyzerError): void;
}

export interface MutationResult<R, P extends Record<string, unknown>> {
  /** Fire and forget; errors land on `.error`. */
  mutate(...args: ParamsArg<P>): void;
  /** Await the result; errors throw. */
  mutateAsync(...args: ParamsArg<P>): Promise<Rows<R>>;
  readonly data: Rows<R> | undefined;
  readonly error: SurrealQLAnalyzerError | undefined;
  readonly pending: boolean;
  reset(): void;
}

/**
 * A write, plus the invalidation that has to follow it. 0.4 had neither, so a
 * non-live cached query stayed stale forever.
 */
export function useMutation<R, P extends Record<string, unknown>>(
  query: SurqlQuery<R, P>,
  options: UseMutationOptions<R> = {},
): MutationResult<R, P> {
  const client = useClient(options.client);
  const [data, setData] = useState<Rows<R> | undefined>(undefined);
  const [error, setError] = useState<SurrealQLAnalyzerError | undefined>(undefined);
  const [pending, setPending] = useState(false);
  // Keep the latest callbacks without making them a dependency of `run`.
  const latest = useRef(options);
  latest.current = options;

  const run = useCallback(
    async (params?: Record<string, unknown>): Promise<Rows<R>> => {
      setPending(true);
      setError(undefined);
      try {
        const result = (await getQueryClient(client).mutate(
          query as unknown as SurqlQuery<R, Record<string, unknown>>,
          params as never,
          { invalidates: latest.current.invalidates },
        )) as Rows<R>;
        setData(() => result);
        latest.current.onSuccess?.(result);
        return result;
      } catch (cause) {
        const wrapped = SurrealQLAnalyzerError.from(cause, { query: query.text, params });
        setError(wrapped);
        latest.current.onError?.(wrapped);
        throw wrapped;
      } finally {
        setPending(false);
      }
    },
    [client, query],
  );

  const mutate = useCallback(
    (...args: ParamsArg<P>) => {
      void run(args[0] as Record<string, unknown> | undefined).catch(() => {
        // Already surfaced on `.error`; swallow so `mutate` never produces an
        // unhandled rejection. Use `mutateAsync` to handle it yourself.
      });
    },
    [run],
  );

  const mutateAsync = useCallback(
    (...args: ParamsArg<P>) => run(args[0] as Record<string, unknown> | undefined),
    [run],
  );

  const reset = useCallback(() => {
    setData(undefined);
    setError(undefined);
    setPending(false);
  }, []);

  return { mutate, mutateAsync, data, error, pending, reset };
}
