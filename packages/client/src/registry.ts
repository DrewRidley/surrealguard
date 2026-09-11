/**
 * The query registry — the contract between `surrealql-analyzer generate` and this
 * client.
 *
 * `surrealql-analyzer generate` emits a module augmentation that adds one entry per
 * analyzed query, keyed by the exact query text:
 *
 * ```ts
 * declare module "@surrealdb/analyzer-client" {
 *   interface SurqlRegistry {
 *     "SELECT * FROM user": {
 *       result: [Array<{ id: RecordId<"user">; name: string }>];
 *       params: Record<string, never>;
 *     };
 *   }
 * }
 * ```
 *
 * A base (empty) interface lives here so `defineQuery` and `db.query` can key
 * on `keyof SurqlRegistry`; the generated file only ever adds entries. Every
 * lookup is one conditional generic on a literal type parameter — there is no
 * permissive `string` overload anywhere on the path, because a literal is also
 * a `string` and a fallback overload would rescue every mis-call.
 */

import type { Jsonify } from "surrealdb";

/**
 * The SurrealDB SDK's own value classes, re-exported so the generated file (and
 * user code) needs a single import. These are the types the SDK **actually
 * decodes** — see {@link Json} for the serialised projection.
 *
 * They are re-exported as values, not merely as types: constructing a parameter
 * (`new RecordId("team", "red")`) is the difference between a query matching
 * and silently returning nothing.
 */
export { Decimal, Duration, RecordId, Uuid } from "surrealdb";
export type { RecordIdValue } from "surrealdb";

/** Minimal GeoJSON shape, matching the codegen convention. */
export type GeoJSON = { type: string; coordinates: unknown };

/**
 * A value as it survives a serialisation boundary — SvelteKit's `load`, a React
 * Server Component's props, `JSON.stringify`. This is the SDK's own `Jsonify`,
 * so the mapping is theirs and not a guess: `RecordId<"team">` becomes
 * `` `team:${string}` ``, and `Date` / `Duration` / `Uuid` / `Decimal` become
 * `string`.
 *
 * The reactive layer (`@surrealdb/analyzer-query` and both framework adapters) is
 * `Json<T>` throughout, because a React Server Component boundary accepts only
 * plain values and offers no transport hook to widen that. `db.run` is not — it
 * hands back the SDK's real values.
 */
export type Json<T> = Jsonify<T>;

/** One registered query's result and parameter types. */
export interface SurqlQueryShape {
  result: unknown;
  params: Record<string, unknown>;
}

/**
 * Every analyzed query, keyed by its exact text. Empty here; the generated
 * declaration file augments it. See the module doc above.
 */
// eslint-disable-next-line @typescript-eslint/no-empty-object-type
export interface SurqlRegistry {}

/** A query whose parameters are all supplied. Adapters accept only these. */
export type Bound = Record<string, never>;

/** The per-statement response tuple a registered query resolves to. */
export type ResultOf<Q extends string> = Q extends keyof SurqlRegistry
  ? SurqlRegistry[Q]["result"]
  : unknown[];

/** The named parameters a registered query reads. */
export type ParamsOf<Q extends string> = Q extends keyof SurqlRegistry
  ? SurqlRegistry[Q]["params"]
  : Record<string, unknown>;

/**
 * The parameters argument for a query: required exactly when the query reads
 * parameters, forbidden (an empty rest) when it reads none, and *optional* for
 * a query whose params are an open record — a non-registered query, where we
 * know nothing and must not demand an object the caller cannot construct.
 *
 * The open-record branch is load-bearing and easy to get wrong: an earlier
 * draft omitted it, and because `Record<string, unknown>` does not extend
 * `Record<string, never>`, every dynamic query then demanded required params.
 */
export type ParamsArg<P> = P extends Record<string, never>
  ? []
  : Record<string, unknown> extends P
    ? [params?: P]
    : [params: P];

/**
 * A readable type error, in the style of Supabase's `ParserError`. Intersecting
 * a branded object with a string *literal* puts the message itself into the
 * compiler's output, so a stale generated file reports what to do about it
 * rather than degrading to `unknown` and failing later, somewhere else.
 */
export type SurqlError<Message extends string> = {
  readonly __surqlError: true;
} & Message;
