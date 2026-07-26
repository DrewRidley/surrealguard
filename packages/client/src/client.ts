/**
 * The typed client.
 *
 * ```ts
 * // src/lib/db.ts
 * import { createClient } from "./surrealguard.generated";
 *
 * export const db = createClient({
 *   url: "ws://localhost:8000/rpc",
 *   namespace: "app",
 *   database: "app",
 * });   // connects lazily; run/watch/query await readiness
 * ```
 * ```ts
 * const people = await db.run(allPeople);            // no destructure
 * const red    = await db.run(peopleOf, { team });   // params checked
 * const stop   = db.watch(livePeople, render);       // live, no other package
 * ```
 *
 * **The client composes the SDK rather than extending it.** `class
 * SurrealGuardClient extends Surreal` does not compile: `Surreal` already owns
 * `run` (RPC function invocation), `subscribe` (the event emitter) and
 * `invalidate` — and `Surreal.invalidate()` *logs the session out*, which makes
 * that collision a hazard rather than an inconvenience. tsc reports TS2416 on
 * each. So the SDK instance is reachable as `db.surreal`, the session methods
 * worth having are forwarded, and our vocabulary stays ours.
 */

import { jsonify, Surreal } from "surrealdb";
import type {
  AccessRecordAuth,
  AnyAuth,
  ConnectOptions,
  DriverOptions,
  LiveSubscription,
  Token,
  Tokens,
} from "surrealdb";
import { SurrealGuardError } from "./error.js";
import { openLive, reconcile, type ReconcilableRow } from "./live.js";
import type { AnyQuery, Rows, SurqlLive, SurqlQuery } from "./query.js";
import type { Bound, Json, ParamsArg, SurqlRegistry } from "./registry.js";

/**
 * The parameters argument for a `db.query(...)` text: required exactly when the
 * query reads parameters, forbidden otherwise. A dynamic string that is not in
 * the registry accepts an optional bindings object. One conditional generic on
 * purpose — a second permissive overload would let a registered query silently
 * skip its required params, so there is none.
 */
export type ArgsOf<Q extends string> = Q extends keyof SurqlRegistry
  ? ParamsArg<SurqlRegistry[Q]["params"]>
  : [bindings?: Record<string, unknown>];

/**
 * What a `db.query(...)` promise resolves to: the SDK's per-statement tuple,
 * one element per statement in source order (`null` for a non-responder such as
 * `LET`/`DEFINE`). A dynamic string resolves to the SDK's `unknown[]`.
 *
 * `db.run` unwraps a single-statement tuple; `db.query` never does. Both are
 * truthful, at different altitudes.
 */
export type QueryResultOf<Q extends string> = Q extends keyof SurqlRegistry
  ? SurqlRegistry[Q]["result"]
  : unknown[];

/** Something that wants to hear about `db.invalidate(...)`. */
export type InvalidationListener = (queries: readonly AnyQuery[]) => void | Promise<void>;

export interface CreateClientOptions extends ConnectOptions, DriverOptions {
  /** Where to connect. The connection opens lazily, on first use. */
  url: string | URL;
}

export interface SurrealGuardClient {
  /**
   * The underlying SDK instance — the escape hatch. Anything the SDK can do and
   * this client does not, do here: `db.surreal.export()`, or
   * ``db.surreal.query(surql`SELECT * FROM ${table}`)`` for a fully dynamic
   * query.
   */
  readonly surreal: Surreal;

  /** Resolves once the connection is open. Every other method awaits it. */
  ready(): Promise<void>;
  /** Close the connection. */
  close(): Promise<void>;
  /** Switch namespace / database. */
  use(what: { namespace?: string | null; database?: string | null }): Promise<void>;
  signin(auth: AnyAuth): Promise<Tokens>;
  signup(auth: AccessRecordAuth): Promise<Tokens>;
  authenticate(token: Token): Promise<void>;

  /**
   * Run a named query. A single-statement query resolves to its result
   * directly; a multi-statement query keeps the per-statement tuple.
   */
  run<R, P extends Record<string, unknown>>(
    query: SurqlQuery<R, P>,
    ...args: ParamsArg<P>
  ): Promise<Rows<R>>;

  /**
   * {@link SurrealGuardClient.run}, projected through {@link Json} — the shape
   * that survives a serialisation boundary. This is what SSR helpers and the
   * reactive layer are built on.
   */
  runJson<R, P extends Record<string, unknown>>(
    query: SurqlQuery<R, P>,
    ...args: ParamsArg<P>
  ): Promise<Json<Rows<R>>>;

  /**
   * Run a live query's underlying `SELECT` once, without subscribing — the SSR
   * seed, and what `preload` is built on.
   */
  runLiveOnce<Row, P extends Record<string, unknown>>(
    query: SurqlLive<Row, P>,
    ...args: ParamsArg<P>
  ): Promise<Row[]>;

  /**
   * Subscribe to a live query. `onRows` receives the whole reconciled array on
   * every change; the returned function unsubscribes.
   *
   * This is what makes `@surrealguard/client` usable on its own. Before, the
   * only way to receive a live row was to install `@surrealguard/query` and
   * discover `getQueryClient(db).observeLive(...).subscribe(...)`.
   */
  watch<Row>(
    query: SurqlLive<Row, Bound>,
    onRows: (rows: Row[]) => void,
    onError?: (error: SurrealGuardError) => void,
  ): () => void;

  /**
   * Announce that the given queries are stale. A reactive core attached to this
   * client refetches its live entries and drops the rest.
   */
  invalidate(...queries: AnyQuery[]): Promise<void>;

  /**
   * Register an invalidation listener. `@surrealguard/query` uses this to
   * attach its cache without the client depending on it — which would be a
   * cycle, since the reactive core is built on the client.
   */
  onInvalidate(listener: InvalidationListener): () => void;

  /**
   * The literal form, unchanged: it resolves the per-statement tuple and
   * requires params exactly when the query reads them, and a dynamic string
   * degrades to `unknown[]` with optional bindings.
   */
  query<Q extends string>(query: Q, ...args: ArgsOf<Q>): Promise<QueryResultOf<Q>>;
}

class GuardClient implements SurrealGuardClient {
  readonly surreal: Surreal;
  readonly #open: (() => Promise<void>) | undefined;
  readonly #listeners = new Set<InvalidationListener>();
  #connecting: Promise<void> | undefined;

  constructor(surreal: Surreal, open?: () => Promise<void>) {
    this.surreal = surreal;
    this.#open = open;
  }

  ready(): Promise<void> {
    const open = this.#open;
    if (!open) return this.surreal.ready;
    this.#connecting ??= open().catch((cause: unknown) => {
      // Let a later call retry rather than caching the failure forever.
      this.#connecting = undefined;
      throw SurrealGuardError.from(cause, {});
    });
    return this.#connecting;
  }

  async close(): Promise<void> {
    this.#connecting = undefined;
    await this.surreal.close();
  }

  async use(what: { namespace?: string | null; database?: string | null }): Promise<void> {
    await this.ready();
    await this.surreal.use(what);
  }

  async signin(auth: AnyAuth): Promise<Tokens> {
    await this.ready();
    return this.surreal.signin(auth);
  }

  async signup(auth: AccessRecordAuth): Promise<Tokens> {
    await this.ready();
    return this.surreal.signup(auth);
  }

  async authenticate(token: Token): Promise<void> {
    await this.ready();
    await this.surreal.authenticate(token);
  }

  /** The one place a query is actually sent, so the one place context is added. */
  async #execute(text: string, params: Record<string, unknown> | undefined): Promise<unknown[]> {
    await this.ready();
    try {
      return (await this.surreal.query(text, params)) as unknown[];
    } catch (cause) {
      throw SurrealGuardError.from(cause, { query: text, params });
    }
  }

  async run(
    query: SurqlQuery<unknown, Record<string, unknown>>,
    params?: Record<string, unknown>,
  ): Promise<never> {
    const results = await this.#execute(query.text, mergeParams(query.params, params));
    return unwrap(results) as never;
  }

  async runJson(
    query: SurqlQuery<unknown, Record<string, unknown>>,
    params?: Record<string, unknown>,
  ): Promise<never> {
    return jsonify(await this.run(query, params)) as never;
  }

  async runLiveOnce(
    query: SurqlLive<unknown, Record<string, unknown>>,
    params?: Record<string, unknown>,
  ): Promise<never> {
    const results = await this.#execute(query.text, mergeParams(query.params, params));
    return (unwrap(results) ?? []) as never;
  }

  watch<Row>(
    query: SurqlLive<Row, Bound>,
    onRows: (rows: Row[]) => void,
    onError?: (error: SurrealGuardError) => void,
  ): () => void {
    let rows: readonly ReconcilableRow[] = [];
    let subscription: LiveSubscription | undefined;
    let stopped = false;

    void (async () => {
      try {
        // Seed from the underlying SELECT first: a LIVE SELECT never replays
        // existing records, so without the seed the list starts empty and stays
        // that way until something changes.
        const seed = await this.#execute(query.text, query.params);
        rows = (unwrap(seed) ?? []) as ReconcilableRow[];
        if (stopped) return;
        onRows(rows as Row[]);

        subscription = await openLive(this.surreal, query.liveText, query.params, (message) => {
          rows = reconcile(rows, message);
          onRows(rows as Row[]);
        });
        if (stopped) void subscription.kill();
      } catch (cause) {
        if (stopped) return;
        const error = SurrealGuardError.from(cause, {
          query: query.liveText,
          params: query.params,
        });
        if (onError) onError(error);
        else throw error;
      }
    })();

    return () => {
      stopped = true;
      if (subscription) void subscription.kill();
      subscription = undefined;
    };
  }

  async invalidate(...queries: AnyQuery[]): Promise<void> {
    await Promise.all([...this.#listeners].map((listener) => listener(queries)));
  }

  onInvalidate(listener: InvalidationListener): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  async query(text: string, bindings?: Record<string, unknown>): Promise<never> {
    return (await this.#execute(text, bindings)) as never;
  }
}

function mergeParams(
  bound: Record<string, unknown> | undefined,
  extra: Record<string, unknown> | undefined,
): Record<string, unknown> | undefined {
  if (!bound) return extra;
  if (!extra) return bound;
  return { ...bound, ...extra };
}

/**
 * Unwrap a single-statement response — the runtime mirror of {@link Rows}. One
 * statement resolves to its own result; anything else keeps the tuple.
 */
function unwrap(results: unknown[]): unknown {
  return results.length === 1 ? results[0] : results;
}

/**
 * Create a client. The connection opens lazily on first use, so a module-level
 * `export const db = createClient(...)` is safe and no route has to remember to
 * `await db.connect(...)`.
 *
 * `codecOptions.useNativeDates` defaults to `true`, which is what makes the
 * generated `Date` type true — without it the SDK decodes its own `DateTime`
 * class and `row.created.getTime()` typechecks and throws at runtime.
 */
export function createClient(options: CreateClientOptions): SurrealGuardClient {
  const { url, engines, codecs, codecOptions, websocketImpl, fetchImpl, ...connectOptions } =
    options;
  const surreal = new Surreal({
    engines,
    codecs,
    codecOptions: { useNativeDates: true, ...codecOptions },
    websocketImpl,
    fetchImpl,
  });
  return new GuardClient(surreal, async () => {
    await surreal.connect(url, connectOptions);
  });
}

/**
 * Wrap a `Surreal` you already own and connected yourself.
 *
 * Construct it with `codecOptions: { useNativeDates: true }`, or the generated
 * `Date` types will be a lie — that is the one thing `createClient` does for you
 * which cannot be recovered afterwards.
 */
export function fromSurreal(surreal: Surreal): SurrealGuardClient {
  return new GuardClient(surreal);
}
