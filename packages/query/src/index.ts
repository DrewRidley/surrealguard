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
 * The client is a {@link SurrealGuardClient} — a real SurrealDB `Surreal`
 * instance. `client.query(...)` resolves to the SDK's per-statement tuple, so
 * one-shot queries unwrap the first statement's rows; a `LIVE SELECT` returns a
 * live-query id which this core subscribes to via `client.liveOf(...)`.
 *
 * The framework packages (`@surrealguard/next`, `@surrealguard/svelte`) are
 * thin bindings over the {@link Observable} returned here.
 */

import type { LiveDescriptor, SurrealGuardClient } from "@surrealguard/client";
import type { LiveMessage, LiveSubscription, Uuid } from "surrealdb";

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
  subscription?: LiveSubscription;
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

  /**
   * Observe a typed {@link LiveDescriptor} produced by `db.live(...)`. The row
   * type is carried from the descriptor's phantom — no runtime constraint, so a
   * degraded (`unknown`) row still works. This is the entry point the framework
   * adapters use.
   */
  observeLive<R>(
    descriptor: LiveDescriptor<R>,
    options: { params?: Record<string, unknown>; initialData?: R[] } = {},
  ): Observable<R> {
    return this.observe(descriptor.sql, {
      params: options.params ?? descriptor.params,
      initialData: options.initialData as Row[] | undefined,
    }) as unknown as Observable<R>;
  }

  /** Run a one-shot query outside the reactive layer (e.g. in SSR `load`). */
  async fetch<R extends Row = Row>(
    sql: string,
    params?: Record<string, unknown>,
  ): Promise<R[]> {
    const [rows] = await this.client.query(sql, params);
    const data = ((rows as R[] | undefined) ?? []) as R[];
    this.commit(queryKey(sql, params), { data, status: "success" });
    return data;
  }

  /**
   * Run a live descriptor's underlying `SELECT` once (stripping `LIVE`) and
   * cache the rows under the live key, so a later {@link observe}/{@link
   * observeLive} of the same query renders gap-free, then upgrades to live. This
   * is the server-side seed used by the framework `load` / server helpers.
   */
  async prime<R>(
    descriptor: LiveDescriptor<R>,
    params?: Record<string, unknown>,
  ): Promise<R[]> {
    const selectSql = descriptor.sql.replace(/^\s*live\s+/i, "");
    const merged = params ?? descriptor.params;
    const [rows] = await this.client.query(selectSql, merged);
    const data = ((rows as R[] | undefined) ?? []) as R[];
    this.commit(queryKey(descriptor.sql, merged), { data: data as Row[], status: "success" });
    return data;
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
      if (isLive(sql)) {
        // A `LIVE SELECT` resolves to its live-query id; subscribe to that id's
        // change stream and reconcile notifications. Seed rows come from
        // `initialData` (SSR), not the live query itself.
        const [liveId] = await this.client.query(sql, params);
        const subscription = await this.client.liveOf(liveId as Uuid);
        entry.subscription = subscription;
        if (entry.state.status === "loading") {
          this.set(entry, { data: entry.state.data, status: "success" });
        }
        subscription.subscribe((message) => this.reconcile(entry, message));
      } else {
        const [rows] = await this.client.query(sql, params);
        this.set(entry, { data: ((rows as Row[] | undefined) ?? []) as Row[], status: "success" });
      }
    } catch (error) {
      this.set(entry, { data: entry.state.data, status: "error", error });
    }
  }

  private stop(_key: string, entry: Entry): void {
    if (entry.subscription) void entry.subscription.kill();
    entry.subscription = undefined;
    entry.started = false;
    // Keep the last data cached for a fast re-subscribe; drop no-longer-live.
  }

  /** Apply a change notification to the reconciled array, keyed by record id. */
  private reconcile(entry: Entry, message: LiveMessage): void {
    if (message.action === "KILLED") return;
    const id = message.recordId;
    const key = String(id);
    const row = { id, ...message.value } as Row;
    const data = entry.state.data.slice();
    const at = data.findIndex((existing) => String(existing.id) === key);
    if (message.action === "DELETE") {
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

/**
 * One {@link QueryClient} per underlying {@link SurrealGuardClient}, cached so
 * every component sharing a client shares its subscription cache — the
 * reference-counting that dedups live subscriptions only works if there is a
 * single reactive core per connection. The framework adapters resolve their
 * reactive core through this.
 */
const queryClients = new WeakMap<object, QueryClient>();

export function getQueryClient(client: SurrealGuardClient): QueryClient {
  let qc = queryClients.get(client);
  if (!qc) {
    qc = new QueryClient(client);
    queryClients.set(client, qc);
  }
  return qc;
}
