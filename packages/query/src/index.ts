/**
 * Framework-agnostic reactive core.
 *
 * A {@link QueryClient} owns a cache of query results keyed by
 * {@link SurqlQuery.key} — the query text plus its stably serialised
 * parameters, computed once on the reference rather than re-derived at every
 * call site. Consumers subscribe through {@link QueryClient.observe} /
 * {@link QueryClient.observeLive}, which reference-count subscriptions so N
 * subscribers to the same query share **one** `LIVE SELECT` and one reconciled
 * array, with a single `KILL` when the last one leaves.
 *
 * A `LIVE SELECT` seeds from its underlying `SELECT` (a live query never
 * replays existing records) and then reconciles change notifications by record
 * `id`. A one-shot query resolves once, and its result is whatever the query
 * returns — an array, a single record, or a scalar.
 *
 * **Everything here is {@link Json}-shaped.** Rows are passed through the SDK's
 * `jsonify` on the way in, so the reactive row type is the one that survives
 * SvelteKit's `load` and a React Server Component's props boundary. `db.run`
 * gives you the SDK's real values; this layer gives you their JSON projection.
 *
 * The framework packages (`@surrealdb/analyzer-next`, `@surrealdb/analyzer-svelte`) are thin
 * bindings over the {@link Observable} returned here.
 */

import { jsonify } from "surrealdb";
import type { LiveSubscription } from "surrealdb";
import {
  reconcile,
  SurrealQLAnalyzerError,
  type AnyQuery,
  type Bound,
  type Json,
  type ParamsArg,
  type QueryKey,
  type ReconcilableRow,
  type Rows,
  type SurqlLive,
  type SurqlQuery,
  type SurrealQLAnalyzerClient,
} from "@surrealdb/analyzer-client";

export type QueryStatus = "pending" | "success" | "error";

/**
 * A real discriminated union, so `status` narrows `data`. The 0.4 shape
 * (`{ data: Row[]; status; error? }`) let you read `data` while still loading
 * and get a silently-empty array; this makes that a type error.
 */
export type QueryState<T> =
  | { status: "pending"; data: T | undefined; error: undefined }
  | { status: "success"; data: T; error: undefined }
  | { status: "error"; data: T | undefined; error: SurrealQLAnalyzerError };

/** A read-only reactive value with subscribe/get. */
export interface Observable<T> {
  get(): QueryState<T>;
  subscribe(listener: (state: QueryState<T>) => void): () => void;
  refetch(): Promise<void>;
}

export interface ObserveOptions<T> {
  /** Seed data (e.g. from SSR hydration) so the first render has no gap. */
  initialData?: T;
}

export interface QueryClientOptions {
  /**
   * How long an entry survives after its last subscriber leaves, in ms. The 0.4
   * cache never evicted at all ("keep the last data for a fast re-subscribe"),
   * so a long-lived SPA with parameterised queries grew without bound.
   * Default: 5 minutes. `Infinity` restores the old behaviour.
   */
  gcTime?: number;
  /**
   * Hard cap on cached entries. When exceeded, the least recently used entries
   * with no subscribers are dropped. Default: 500.
   */
  maxEntries?: number;
}

interface Entry {
  key: string;
  /** The bare text: what a one-shot query runs, and what a live query seeds with. */
  text: string;
  /** The `LIVE SELECT …` text, for a live entry. */
  liveText: string | undefined;
  params: Record<string, unknown> | undefined;
  isLive: boolean;
  state: QueryState<unknown>;
  listeners: Set<(state: QueryState<unknown>) => void>;
  refs: number;
  subscription: LiveSubscription | undefined;
  started: boolean;
  lastUsed: number;
  gcTimer: ReturnType<typeof setTimeout> | undefined;
}

export interface DehydratedState {
  [key: string]: unknown;
}

const DEFAULT_GC_TIME = 5 * 60_000;
const DEFAULT_MAX_ENTRIES = 500;

export class QueryClient {
  readonly #client: SurrealQLAnalyzerClient;
  readonly #cache = new Map<string, Entry>();
  readonly #gcTime: number;
  readonly #maxEntries: number;

  constructor(client: SurrealQLAnalyzerClient, options: QueryClientOptions = {}) {
    this.#client = client;
    this.#gcTime = options.gcTime ?? DEFAULT_GC_TIME;
    this.#maxEntries = options.maxEntries ?? DEFAULT_MAX_ENTRIES;
    // `db.invalidate(...)` has to reach this cache, but the client cannot
    // import it (the reactive core is built *on* the client), so the client
    // publishes and we subscribe.
    client.onInvalidate((queries) => this.invalidate(...queries));
  }

  /**
   * Observe a one-shot query. The result is whatever the query returns — an
   * array, one record, or a scalar. 0.4 forced `Row[]` here, which meant a
   * `RETURN count(…)` could not be observed at all.
   */
  observe<R>(
    query: SurqlQuery<R, Bound>,
    options: ObserveOptions<Json<Rows<R>>> = {},
  ): Observable<Json<Rows<R>>> {
    return this.#entryObservable(this.#ensure(query), options) as Observable<Json<Rows<R>>>;
  }

  /**
   * Observe a live query. `data` is always an array — a live query is a row
   * stream — and starts `[]`, so markup needs no `?? []`.
   */
  observeLive<Row>(
    query: SurqlLive<Row, Bound>,
    options: ObserveOptions<Json<Row>[]> = {},
  ): Observable<Json<Row>[]> {
    // No `?? []` default here: an empty array would read as "already
    // successful" and suppress the seed fetch. A live entry starts *pending*
    // with `data: []` instead, which is both honest and gives markup an array
    // from the first render.
    return this.#entryObservable(this.#ensure(query), options) as Observable<Json<Row>[]>;
  }

  /** Run a query outside the reactive layer, and cache the result. */
  async fetch<R>(query: SurqlQuery<R, Bound>): Promise<Json<Rows<R>>> {
    const entry = this.#ensure(query);
    await this.#load(entry);
    return entry.state.data as Json<Rows<R>>;
  }

  /**
   * Run a live query's underlying `SELECT` once and cache the rows under the
   * live key, so a later {@link observeLive} renders gap-free and then upgrades
   * to live. This is the server-side seed.
   */
  async prime<Row>(query: SurqlLive<Row, Bound>): Promise<Json<Row>[]> {
    const entry = this.#ensure(query);
    await this.#load(entry);
    return entry.state.data as Json<Row>[];
  }

  /**
   * Read a cached result. The key is branded with its result type (TanStack's
   * `DataTag` trick), so this needs no annotation and cannot be asked for the
   * wrong type.
   */
  getData<T>(key: QueryKey<T>): T | undefined {
    return this.#cache.get(key)?.state.data as T | undefined;
  }

  /** Write a cached result — an optimistic update, before the write lands. */
  setData<R>(
    query: SurqlQuery<R, Bound>,
    updater: (previous: Json<Rows<R>> | undefined) => Json<Rows<R>>,
  ): void {
    const entry = this.#ensure(query);
    this.#set(entry, {
      status: "success",
      data: updater(entry.state.data as Json<Rows<R>> | undefined),
      error: undefined,
    });
  }

  /** Re-run a query now, whether or not anything is subscribed. */
  async refetch(query: AnyQuery): Promise<void> {
    const entry = this.#cache.get(query.key);
    if (entry) await this.#load(entry);
  }

  /**
   * Mark queries stale. An entry with subscribers is refetched in place; one
   * without is dropped, so the next subscriber gets fresh data.
   *
   * Matching is by key **prefix**, which is what makes the two granularities in
   * the API real: an unbound query's key is its text, and a bound one's is
   * `text::params`. So `invalidate(peopleOf)` hits every binding, and
   * `invalidate(peopleOf.with({ team }))` hits exactly one.
   */
  async invalidate(...queries: readonly AnyQuery[]): Promise<void> {
    const pending: Promise<void>[] = [];
    for (const entry of [...this.#cache.values()]) {
      if (!queries.some((query) => matches(entry.key, query.key))) continue;
      if (entry.refs > 0) pending.push(this.#load(entry));
      else this.#drop(entry);
    }
    await Promise.all(pending);
  }

  /**
   * Run a write and invalidate what it affects. The invalidation targets do not
   * have to agree with the mutation's own types — that is why they are
   * {@link AnyQuery}.
   */
  async mutate<R, P extends Record<string, unknown>>(
    query: SurqlQuery<R, P>,
    ...args: [...params: ParamsArg<P>, options?: { invalidates?: readonly AnyQuery[] }]
  ): Promise<Rows<R>> {
    const [params, options] = splitMutateArgs(args);
    const result = await (
      this.#client.run as (
        q: SurqlQuery<R, Record<string, unknown>>,
        p?: Record<string, unknown>,
      ) => Promise<Rows<R>>
    )(query as SurqlQuery<R, Record<string, unknown>>, params);
    if (options?.invalidates?.length) await this.invalidate(...options.invalidates);
    return result;
  }

  /** Snapshot cached results for transport to the client (SSR). */
  dehydrate(): DehydratedState {
    const out: DehydratedState = {};
    for (const [key, entry] of this.#cache) {
      if (entry.state.status === "success") out[key] = entry.state.data;
    }
    return out;
  }

  /** Seed the cache from a server snapshot so the client avoids a refetch. */
  hydrate(state: DehydratedState): void {
    for (const key of Object.keys(state)) {
      const entry = this.#cache.get(key);
      const next: QueryState<unknown> = {
        status: "success",
        data: state[key],
        error: undefined,
      };
      if (entry) this.#set(entry, next);
      else this.#cache.set(key, blankEntry(key, key, undefined, undefined, false, next));
    }
  }

  /** Drop everything. Mostly for tests and for a sign-out. */
  clear(): void {
    for (const entry of [...this.#cache.values()]) this.#drop(entry);
  }

  /**
   * Throw away everything the current identity saw, and re-run whatever is
   * still on screen. Call this after `signin` / `signup` / `authenticate` /
   * `db.surreal.invalidate()`.
   *
   * {@link clear} is not enough on its own, and the difference is a security
   * one rather than a nicety. A cache key is the query text plus its
   * parameters; `$auth` appears in neither, so two identities share a key. An
   * entry with subscribers is held by the observable those subscribers closed
   * over, so deleting it from the map leaves the old rows on screen *and*
   * silently kills their subscription — one user's data, rendered to the next
   * one, no longer updating. So subscribed entries are reset in place instead:
   * their `LIVE SELECT` is killed (its permission context was captured under
   * the old identity and cannot be re-pointed), their state goes back to
   * pending, and they start again under the new one.
   */
  async reset(): Promise<void> {
    const restarted: Promise<void>[] = [];
    for (const entry of [...this.#cache.values()]) {
      if (entry.refs === 0) {
        this.#drop(entry);
        continue;
      }
      this.#kill(entry);
      // Back to pending, so `#start` actually reloads rather than trusting the
      // previous identity's rows, and so markup showing a spinner shows one.
      this.#set(entry, {
        status: "pending",
        data: entry.isLive ? [] : undefined,
        error: undefined,
      });
      entry.started = true;
      restarted.push(this.#start(entry));
    }
    await Promise.all(restarted);
  }

  // --- internals ----------------------------------------------------------

  #ensure(query: SurqlQuery<unknown, Bound> | SurqlLive<unknown, Bound>): Entry {
    let entry = this.#cache.get(query.key);
    if (!entry) {
      entry = blankEntry(
        query.key,
        query.text,
        query.isLive ? (query as SurqlLive<unknown, Bound>).liveText : undefined,
        query.params,
        query.isLive,
        // A live query is a row stream, so its pending data is an empty array
        // rather than `undefined` — markup never needs `?? []`.
        { status: "pending", data: query.isLive ? [] : undefined, error: undefined },
      );
      this.#cache.set(query.key, entry);
      this.#evict();
    }
    entry.lastUsed = Date.now();
    if (entry.gcTimer) {
      clearTimeout(entry.gcTimer);
      entry.gcTimer = undefined;
    }
    return entry;
  }

  #entryObservable(entry: Entry, options: ObserveOptions<unknown>): Observable<unknown> {
    if (options.initialData !== undefined && entry.state.status === "pending") {
      entry.state = { status: "success", data: options.initialData, error: undefined };
    }
    return {
      get: () => entry.state,
      refetch: () => this.#load(entry),
      subscribe: (listener) => {
        entry.listeners.add(listener);
        entry.refs += 1;
        entry.lastUsed = Date.now();
        if (entry.gcTimer) {
          clearTimeout(entry.gcTimer);
          entry.gcTimer = undefined;
        }
        if (!entry.started) {
          entry.started = true;
          void this.#start(entry);
        }
        listener(entry.state);
        return () => {
          entry.listeners.delete(listener);
          entry.refs -= 1;
          if (entry.refs === 0) this.#stop(entry);
        };
      },
    };
  }

  /** Fetch (or seed) an entry's data once. */
  async #load(entry: Entry): Promise<void> {
    try {
      const result = await this.#client.query(entry.text, entry.params);
      const data = jsonify(unwrap(result as unknown[]));
      this.#set(entry, {
        status: "success",
        data: entry.isLive ? (data ?? []) : data,
        error: undefined,
      });
    } catch (cause) {
      this.#fail(entry, cause, entry.text);
    }
  }

  /** Start an entry: fetch once, and for a live query, subscribe. */
  async #start(entry: Entry): Promise<void> {
    // A live query never replays existing records, so seed from the bare SELECT
    // unless SSR already handed us rows.
    if (entry.state.status !== "success") await this.#load(entry);
    if (!entry.isLive || !entry.liveText) return;
    // If the seed failed, don't open a subscription on top of it — doing so
    // overwrites the error the caller needs to see with whatever the LIVE
    // attempt reports (usually the same failure, described worse).
    if (entry.state.status === "error") return;
    try {
      const { openLive } = await import("@surrealdb/analyzer-client");
      const subscription = await openLive(
        this.#client.surreal,
        entry.liveText,
        entry.params,
        (message) => {
          const rows = (entry.state.data ?? []) as ReconcilableRow[];
          this.#set(entry, {
            status: "success",
            data: reconcile(rows, message, (row) => jsonify(row) as ReconcilableRow),
            error: undefined,
          });
        },
      );
      if (entry.refs === 0) void subscription.kill();
      else entry.subscription = subscription;
    } catch (cause) {
      this.#fail(entry, cause, entry.liveText);
    }
  }

  /**
   * Drop an entry's subscription, tolerating a `KILL` the server refuses.
   *
   * That is not hypothetical: signing in as someone else invalidates the
   * session's live queries server-side, so the `KILL` that follows reports
   * "Cannot execute KILL statement using id: …". The subscription is gone
   * either way; an unhandled rejection on top of it helps nobody.
   */
  #kill(entry: Entry): void {
    entry.subscription?.kill().catch(() => {
      // Already gone.
    });
    entry.subscription = undefined;
  }

  #stop(entry: Entry): void {
    this.#kill(entry);
    entry.started = false;
    if (this.#gcTime === Infinity) return;
    const timer = setTimeout(() => {
      if (entry.refs === 0) this.#cache.delete(entry.key);
    }, this.#gcTime);
    // Never hold a Node process open just for a cache eviction. (Browsers
    // return a number from setTimeout, which has no `unref`.)
    (timer as { unref?: () => void }).unref?.();
    entry.gcTimer = timer;
  }

  #drop(entry: Entry): void {
    this.#kill(entry);
    if (entry.gcTimer) clearTimeout(entry.gcTimer);
    this.#cache.delete(entry.key);
  }

  /** Bound the cache: drop the least recently used unsubscribed entries. */
  #evict(): void {
    if (this.#cache.size <= this.#maxEntries) return;
    const idle = [...this.#cache.values()]
      .filter((entry) => entry.refs === 0)
      .sort((a, b) => a.lastUsed - b.lastUsed);
    for (const entry of idle) {
      if (this.#cache.size <= this.#maxEntries) break;
      this.#drop(entry);
    }
  }

  #fail(entry: Entry, cause: unknown, text: string): void {
    this.#set(entry, {
      status: "error",
      data: entry.state.data,
      error: SurrealQLAnalyzerError.from(cause, { query: text, params: entry.params }),
    });
  }

  #set(entry: Entry, state: QueryState<unknown>): void {
    entry.state = state;
    for (const listener of entry.listeners) listener(state);
  }
}

function blankEntry(
  key: string,
  text: string,
  liveText: string | undefined,
  params: Record<string, unknown> | undefined,
  isLive: boolean,
  state: QueryState<unknown>,
): Entry {
  return {
    key,
    text,
    liveText,
    params,
    isLive,
    state,
    listeners: new Set(),
    refs: 0,
    subscription: undefined,
    started: false,
    lastUsed: Date.now(),
    gcTimer: undefined,
  };
}

/** `text::a=1` is a binding of `text`; `text` alone matches every binding. */
function matches(entryKey: string, targetKey: string): boolean {
  return entryKey === targetKey || entryKey.startsWith(`${targetKey}::`);
}

/** The runtime mirror of `Rows<R>`: one statement resolves to its own result. */
function unwrap(results: unknown[]): unknown {
  return results.length === 1 ? results[0] : results;
}

function splitMutateArgs(
  args: readonly unknown[],
): [Record<string, unknown> | undefined, { invalidates?: readonly AnyQuery[] } | undefined] {
  const [first, second] = args as [unknown, unknown];
  if (second !== undefined) {
    return [
      first as Record<string, unknown>,
      second as { invalidates?: readonly AnyQuery[] },
    ];
  }
  if (first && typeof first === "object" && "invalidates" in first) {
    return [undefined, first as { invalidates?: readonly AnyQuery[] }];
  }
  return [first as Record<string, unknown> | undefined, undefined];
}

/**
 * One {@link QueryClient} per underlying client, cached — the reference
 * counting that dedups live subscriptions only works if there is a single
 * reactive core per connection.
 */
const queryClients = new WeakMap<object, QueryClient>();

export function getQueryClient(client: SurrealQLAnalyzerClient): QueryClient {
  let queryClient = queryClients.get(client);
  if (!queryClient) {
    queryClient = new QueryClient(client);
    queryClients.set(client, queryClient);
  }
  return queryClient;
}

export type { AnyQuery, Json, QueryKey, Rows, SurqlLive, SurqlQuery };
export { SurrealQLAnalyzerError };
