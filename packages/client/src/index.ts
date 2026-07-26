/**
 * `@surrealguard/client` — the typed SurrealQL client.
 *
 * The whole guarantee lives in one mechanism: `surrealguard generate` emits an
 * `interface SurqlRegistry` keyed by *exact query text*, and a single
 * conditional generic reads it. There is no permissive `string` overload
 * anywhere (a literal is also a `string`, so a fallback overload would rescue
 * every mis-call into `unknown`), and a miss degrades to `unknown`, never `any`.
 */

export {
  Decimal,
  Duration,
  RecordId,
  Uuid,
  type Bound,
  type GeoJSON,
  type Json,
  type ParamsArg,
  type ParamsOf,
  type RecordIdValue,
  type ResultOf,
  type SurqlError,
  type SurqlQueryShape,
  type SurqlRegistry,
} from "./registry.js";

export {
  computeKey,
  defineLive,
  defineQuery,
  type AnyQuery,
  type DefinedLive,
  type DefinedQuery,
  type QueryKey,
  type RowOf,
  type Rows,
  type SurqlLive,
  type SurqlQuery,
} from "./query.js";

export {
  createClient,
  fromSurreal,
  type ArgsOf,
  type CreateClientOptions,
  type InvalidationListener,
  type QueryResultOf,
  type SurrealGuardClient,
} from "./client.js";

export { SurrealGuardError, type SurrealGuardErrorContext } from "./error.js";

export { openLive, reconcile, type ReconcilableRow } from "./live.js";

export { preload, type Preloaded } from "./preload.js";
