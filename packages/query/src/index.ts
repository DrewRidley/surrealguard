/**
 * Framework-agnostic reactive core.
 *
 * A {@link QueryClient} owns a cache of live/one-shot query results keyed by
 * `(sql, params)`. Components subscribe through {@link QueryClient.observe},
 * which reference-counts subscriptions so N subscribers to the same query share
 * one live subscription and one reconciled array. A `LIVE SELECT` opens a
 * subscription and reconciles change notifications by record `id`; a plain
 * query resolves once. {@link QueryClient.dehydrate}/{@link QueryClient.hydrate}
 * carry server-fetched data across the SSR boundary so the client renders
 * without a refetch, then (for live queries) upgrades in place.
 *
 * The framework packages (`@surrealguard/next`, `@surrealguard/svelte`) are
 * thin bindings over the {@link Observable} returned here.
 */

import type { LiveNotification, SurrealGuardClient } from "@surrealguard/client";

export type QueryStatus = "loading" | "success" | "error";

export interface QueryState<Row> {
  data: Row[];
  status: QueryStatus;
  error?: unknown;
}

/** A read-only reactive value with subscribe/get and a release handle. */
export interface Observable<Row> {
  get(): QueryState<Row>;
  subscribe(listener: (state: QueryState<Row>) => void): () => void;
}

export interface ObserveOptions<Row> {
  params?: Record<string, unknown>;
  /** Seed data (e.g. from SSR hydration) so the first render has no gap. */
  initialData?: Row[];
}

type Row = Record<string, unknown> & { id?: unknown };

/** Stable key for a query + params, so identical calls share a cache entry. */
export function queryKey(sql: string, params?: Record<string, unknown>): string {
  if (!params || Object.keys(params).length === 0) return sql;
  const stable = Object.keys(params)
    .sort()
    .map((k) => `${k}=${JSON.stringify(params[k])}`)
    .join("&");
  return `${sql}::${stable}`;
}

function isLive(sql: string): boolean {
  return /^\s*live\b/i.test(sql);
}

interface Entry {
  state: QueryState<Row>;
  listeners: Set<(state: QueryState<Row>) => void>;
  refs: number;
  liveId?: string;
  started: boolean;
}

export interface DehydratedState {
  [key: string]: Row[];
}

export class QueryClient {
  private readonly cache = new Map<string, Entry>();

  constructor(private readonly client: SurrealGuardClient) {}

  /**
   * Observe a query. Reference-counted: the first subscriber starts it (a
   * one-shot fetch, or a live subscription that reconciles by id); the last to
   * leave tears the live subscription down. Returns a reactive handle.
   */
  observe<R extends Row = Row>(sql: string, options: ObserveOptions<R> = {}): Observable<R> {
    const key = queryKey(sql, options.params);
    let entry = this.cache.get(key);
    if (!entry) {
      entry = {
        state: {
          data: (options.initialData as Row[] | undefined) ?? [],
          status: options.initialData ? "success" : "loading",
        },
        listeners: new Set(),
        refs: 0,
        started: false,
      };
      this.cache.set(key, entry);
    } else if (options.initialData && entry.state.data.length === 0) {
      entry.state = { data: options.initialData as Row[], status: "success" };
    }

    const current = entry;
    return {
      get: () => current.state as QueryState<R>,
      subscribe: (listener) => {
        const typed = listener as (state: QueryState<Row>) => void;
        current.listeners.add(typed);
        current.refs += 1;
        if (!current.started) {
          current.started = true;
          void this.start(sql, options.params, current);
        }
        listener(current.state as QueryState<R>);
        return () => {
          current.listeners.delete(typed);
          current.refs -= 1;
          if (current.refs === 0) this.stop(key, current);
        };
      },
    };
  }

  /** Run a one-shot query outside the reactive layer (e.g. in SSR `load`). */
  async fetch<R extends Row = Row>(
    sql: string,
    params?: Record<string, unknown>,
  ): Promise<R[]> {
    const result = (await this.client.query(sql, params as never)) as R[];
    this.commit(queryKey(sql, params), { data: result, status: "success" });
    return result;
  }

  /** Snapshot cached results for transport to the client (SSR). */
  dehydrate(): DehydratedState {
    const out: DehydratedState = {};
    for (const [key, entry] of this.cache) {
      if (entry.state.status === "success") out[key] = entry.state.data;
    }
    return out;
  }

  /** Seed the cache from a server snapshot so the client avoids a refetch. */
  hydrate(state: DehydratedState): void {
    for (const key of Object.keys(state)) {
      const data = state[key] ?? [];
      const entry = this.cache.get(key);
      if (entry) {
        entry.state = { data, status: "success" };
      } else {
        this.cache.set(key, {
          state: { data, status: "success" },
          listeners: new Set(),
          refs: 0,
          started: false,
        });
      }
    }
  }

  private async start(
    sql: string,
    params: Record<string, unknown> | undefined,
    entry: Entry,
  ): Promise<void> {
    try {
      if (isLive(sql) && this.client.raw.live) {
        // Live: seed with the current rows, then reconcile notifications.
        const initial = (await this.client.query(sql, params as never)) as unknown;
        if (Array.isArray(initial)) this.set(entry, { data: initial as Row[], status: "success" });
        entry.liveId = await this.client.raw.live(sql, (note) => this.reconcile(entry, note));
      } else {
        const data = (await this.client.query(sql, params as never)) as Row[];
        this.set(entry, { data, status: "success" });
      }
    } catch (error) {
      this.set(entry, { data: entry.state.data, status: "error", error });
    }
  }

  private stop(key: string, entry: Entry): void {
    if (entry.liveId && this.client.raw.kill) void this.client.raw.kill(entry.liveId);
    entry.liveId = undefined;
    entry.started = false;
    // Keep the last data cached for a fast re-subscribe; drop no-longer-live.
  }

  /** Apply a change notification to the reconciled array, keyed by `id`. */
  private reconcile(entry: Entry, note: LiveNotification): void {
    const row = note.result as Row;
    const id = row.id;
    const data = entry.state.data.slice();
    const at = data.findIndex((existing) => existing.id === id);
    if (note.action === "DELETE") {
      if (at >= 0) data.splice(at, 1);
    } else if (at >= 0) {
      data[at] = row;
    } else {
      data.push(row);
    }
    this.set(entry, { data, status: "success" });
  }

  private set(entry: Entry, state: QueryState<Row>): void {
    entry.state = state;
    for (const listener of entry.listeners) listener(state);
  }

  private commit(key: string, state: QueryState<Row>): void {
    const entry = this.cache.get(key);
    if (entry) this.set(entry, state);
    else
      this.cache.set(key, {
        state,
        listeners: new Set(),
        refs: 0,
        started: false,
      });
  }
}
