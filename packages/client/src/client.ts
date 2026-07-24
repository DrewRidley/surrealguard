/**
 * The typed client. `SurrealGuardClient` **is** a SurrealDB `Surreal` instance —
 * it extends the official SDK class, so every SDK method is available and
 * `new SurrealGuardClient()` / `await db.connect(...)` behaves exactly like the
 * SDK. On top of that, a string-literal `db.query(...)` resolves its result and
 * parameters from the generated {@link SurqlRegistry}.
 *
 * ```ts
 * import { SurrealGuardClient } from "./surrealguard.generated";
 *
 * const db = new SurrealGuardClient();
 * await db.connect("ws://localhost:8000/rpc");
 *
 * // Result + params inferred from the query text. The SDK's `query` resolves
 * // to the per-statement tuple, so destructure the first statement's result:
 * const [users] = await db.query("SELECT * FROM user");
 * const [team] = await db.query("SELECT * FROM user WHERE team = $team", { team: "red" });
 * ```
 */

import { Surreal, type BoundQuery, type LiveResource } from "surrealdb";
import type { ParamsArg, SurqlRegistry } from "./registry.js";
import { buildLiveSql, ensureLive, type LiveDescriptor, type LiveRowOf } from "./live.js";

/**
 * The parameters argument for a query text: required exactly when the query
 * reads parameters, forbidden (an empty rest) otherwise. Dynamic strings that
 * are not in the registry accept an optional bindings object. This is one
 * conditional generic on purpose — a second permissive overload would let a
 * registered query silently skip its required params, so there is none.
 */
export type ArgsOf<Q extends string> = Q extends keyof SurqlRegistry
  ? ParamsArg<SurqlRegistry[Q]["params"]>
  : [bindings?: Record<string, unknown>];

/**
 * The value a `db.query(...)` promise resolves to. SurrealDB returns one result
 * per statement, so a registered (single-statement) query resolves to a
 * one-element tuple carrying its result; a dynamic string resolves to the
 * SDK's `unknown[]`.
 */
export type QueryResultOf<Q extends string> = Q extends keyof SurqlRegistry
  ? [SurqlRegistry[Q]["result"]]
  : unknown[];

/**
 * The SDK's fluent query builder, named indirectly because the SDK does not
 * export the `Query` class. Intersecting it with `Promise<T>` narrows what
 * `await` yields while staying assignable to the builder the base method
 * returns — so the override type-checks and the builder's `.retry()` /
 * `.collect()` / `.stream()` chain stays available on typed queries.
 */
type QueryBuilder = ReturnType<Surreal["query"]>;

export class SurrealGuardClient extends Surreal {
  /**
   * Runs a query. For a string literal that matches a generated registry
   * entry, the result tuple and the params argument are inferred from the query
   * text; params are required exactly when the query reads them. Dynamic
   * strings resolve to the SDK's `unknown[]` with optional bindings. A
   * `BoundQuery` passes straight through to the SDK.
   */
  query<Q extends string>(query: Q, ...args: ArgsOf<Q>): QueryBuilder & Promise<QueryResultOf<Q>>;
  query<R extends unknown[] = unknown[]>(query: BoundQuery<R>): QueryBuilder & Promise<R>;
  query(query: string | BoundQuery, bindings?: Record<string, unknown>): QueryBuilder {
    return (
      typeof query === "string" ? super.query(query, bindings) : super.query(query)
    ) as QueryBuilder;
  }

  /**
   * Describe a live query. Passed the query as a string / template-literal
   * **argument** — `db.live(\`SELECT * FROM user\`)` — the row type is inferred
   * from the generated registry exactly like {@link SurrealGuardClient.query} (a
   * non-registered query degrades to `unknown`, never `any`), and the returned
   * {@link LiveDescriptor} carries the `LIVE SELECT …` text for the framework
   * adapters to observe. Written without the `LIVE` prefix; it is added
   * automatically.
   *
   * Note the parentheses: this takes a string argument, not a *tagged* template.
   * TypeScript widens a tagged template's cooked text to `string`, which would
   * lose the row type, so `db.live(\`…\`)` / `db.live("…")` are the typed forms.
   * A bare tagged call `db.live\`…\`` still runs but its row is `unknown`.
   *
   * The SDK's own `live(table)` subscription remains available — a non-string,
   * non-template (table / record) argument passes straight through to it.
   */
  live<Q extends string>(query: Q): LiveDescriptor<LiveRowOf<Q>>;
  live(strings: TemplateStringsArray, ...values: unknown[]): LiveDescriptor<unknown>;
  live<T>(what: LiveResource): ReturnType<Surreal["live"]>;
  live(
    arg: string | TemplateStringsArray | LiveResource,
    ...values: unknown[]
  ): LiveDescriptor | ReturnType<Surreal["live"]> {
    if (typeof arg === "string") return { sql: ensureLive(arg) };
    if (Array.isArray(arg)) return { sql: buildLiveSql(arg, values) };
    return super.live(arg as LiveResource);
  }
}
