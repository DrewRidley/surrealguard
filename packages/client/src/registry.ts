/**
 * The query registry — the contract between `surrealguard generate` and this
 * client.
 *
 * `surrealguard generate` emits a module augmentation that adds one entry per
 * analyzed query, keyed by the exact query text:
 *
 * ```ts
 * declare module "@surrealguard/client" {
 *   interface SurqlRegistry {
 *     "SELECT * FROM user": {
 *       result: Array<{ id: RecordId<"user">; name: string }>;
 *       params: Record<string, never>;
 *     };
 *   }
 * }
 * ```
 *
 * A base (empty) interface lives here so the client's `query` method can key on
 * `keyof SurqlRegistry`; the generated file only ever adds entries.
 */

/** A branded record id: a string that remembers its table at the type level. */
export type RecordId<T extends string = string> = string & { readonly __table?: T };

/** Minimal GeoJSON shape, matching the codegen convention. */
export type GeoJSON = { type: string; coordinates: unknown };

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

/**
 * The parameters argument for a query: required exactly when the query reads
 * parameters, forbidden (an empty rest) otherwise. This is the single trick
 * that makes `db.query("...")` type-safe without a wrapper — proven against
 * tsc: a second permissive overload would defeat it, so `query` stays one
 * conditional generic.
 */
export type ParamsArg<P> = P extends Record<string, never> ? [] : [params: P];
