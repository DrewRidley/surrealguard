/**
 * Next.js / React bindings for SurrealGuard (client entry — `"use client"`).
 *
 * - {@link SurrealGuardProvider} / {@link useClient} — provide the typed client.
 * - {@link useLiveQuery} — live queries as reactive `{ data, status, error }`.
 *
 * Server-side seed helpers (`queryServer`, `dehydrate`, `hydrate`) live in the
 * separate, server-safe entry `@surrealguard/next/server`.
 */

export {
  SurrealGuardProvider,
  useClient,
  type SurrealGuardProviderProps,
} from "./context.js";
export { useLiveQuery, type UseLiveQueryOptions } from "./use-live-query.js";
