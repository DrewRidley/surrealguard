/**
 * The typed client. Wraps any SurrealDB-like connection and exposes a `query`
 * method whose result and parameters are resolved from {@link SurqlRegistry}
 * for string-literal queries, and fall back to `unknown` for dynamic strings.
 */

import type { ParamsArg, SurqlRegistry } from "./registry.js";

/** A live-query change notification, as SurrealDB delivers them. */
export interface LiveNotification<Row = Record<string, unknown>> {
  action: "CREATE" | "UPDATE" | "DELETE";
  result: Row;
}

/**
 * The transport this client drives. Satisfied by the SurrealDB JS SDK's
 * `Surreal` instance (adapt with {@link fromSurreal}) or any equivalent.
 */
export interface Connection {
  /** Run a query, returning the first statement's result. */
  query(sql: string, vars?: Record<string, unknown>): Promise<unknown>;
  /** Open a live query, returning its id; notifications go to `cb`. */
  live?(sql: string, cb: (n: LiveNotification) => void): Promise<string>;
  /** Terminate a live query by id. */
  kill?(id: string): Promise<void>;
}

/** The result type a registered query yields, or `unknown` for a dynamic one. */
export type ResultOf<Q extends string> = Q extends keyof SurqlRegistry
  ? SurqlRegistry[Q]["result"]
  : unknown;

/** The params argument tuple for a query. */
export type ArgsOf<Q extends string> = Q extends keyof SurqlRegistry
  ? ParamsArg<SurqlRegistry[Q]["params"]>
  : [params?: Record<string, unknown>];

export class SurrealGuardClient {
  constructor(private readonly connection: Connection) {}

  /**
   * Runs a query. For a string literal that matches a generated registry
   * entry, the result type and the params argument are inferred from the query
   * text; params are required exactly when the query reads them. Dynamic
   * strings resolve to `unknown` with optional params.
   */
  query<Q extends string>(query: Q, ...args: ArgsOf<Q>): Promise<ResultOf<Q>> {
    const params = args[0] as Record<string, unknown> | undefined;
    return this.connection.query(query, params) as Promise<ResultOf<Q>>;
  }

  /** The underlying connection, for live queries and lower-level access. */
  get raw(): Connection {
    return this.connection;
  }
}

/**
 * Adapts a SurrealDB JS SDK `Surreal` instance to a {@link Connection}. Kept
 * structural so this package does not depend on the SDK; pass your `Surreal`.
 */
export function fromSurreal(db: {
  query(sql: string, vars?: Record<string, unknown>): Promise<unknown[]>;
  live?(sql: string, cb: (n: LiveNotification) => void): Promise<string>;
  kill?(id: string): Promise<void>;
}): Connection {
  return {
    async query(sql, vars) {
      // The SDK returns one result per statement; a single-statement query's
      // result is the first entry.
      const results = await db.query(sql, vars);
      return results[0];
    },
    live: db.live?.bind(db),
    kill: db.kill?.bind(db),
  };
}
